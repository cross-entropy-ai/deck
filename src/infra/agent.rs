//! Detect coding agents (Claude Code, Codex) running *interactively* in
//! tmux panes, with no hooks installed — by walking the process tree
//! under each pane's pid and matching the agent's `argv`, recording which
//! tmux session/window/pane each one sits in.
//!
//! See `docs/agent-integration.md` for the research behind the
//! signatures. In short: `pane_current_command` is unreliable (it shows
//! Claude Code's version string, and flips while the agent runs a
//! subprocess), so we look for an agent process anywhere in each pane's
//! subtree.

use std::collections::{HashMap, VecDeque};
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentKind {
    Claude,
    Codex,
}

/// One tmux pane's identity, fed to `detect_agents`. `session`/`window`/
/// `pane` are display fields (`session_name`, `window_index`,
/// `pane_index`); `pane_id` is the stable `%N` handle used for switching
/// (indices renumber as panes/windows come and go, so they must not be
/// the switch target — see the adversarial review).
#[derive(Debug, Clone)]
pub struct PaneInfo {
    pub pid: u32,
    pub session: String,
    pub window: String,
    pub pane: String,
    pub pane_id: String,
}

/// An interactive agent located in a specific pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedAgent {
    pub kind: AgentKind,
    pub session: String,
    pub window: String,
    pub pane: String,
    /// Stable `%N` pane id — the switch/focus target.
    pub pane_id: String,
    /// Traffic-light health, classified from the pane buffer (see
    /// [`StatusClassifier`]). `Unknown` until a buffer is captured and
    /// classified — `detect_agents` itself has no buffer, so it leaves this
    /// `Unknown` for the gathering layer to fill in.
    pub status: AgentStatus,
}

/// Traffic-light health shown as a colored dot before each agent row:
/// green = actively working, red = not working (idle), yellow = waiting for
/// user input, default/uncolored = unknown (not captured, or an agent kind
/// whose classifier isn't implemented yet). Color mapping lives in the
/// renderer (`ui::sidebar::sessions::recolor_agent_dot`), keyed off this
/// status (not the glyph); these stay semantic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AgentStatus {
    Working,
    Idle,
    Waiting,
    #[default]
    Unknown,
}

impl DetectedAgent {
    /// Compact `session:window.pane` location for display.
    pub fn location(&self) -> String {
        format!("{}:{}.{}", self.session, self.window, self.pane)
    }
}

/// A snapshot of an agent's pane, fed to a [`StatusClassifier`]. A struct
/// (not a bare `&str`) so future signals — idle time, exit code, scrollback
/// — can be added without changing every classifier's signature.
pub struct PaneSnapshot<'a> {
    /// The visible pane buffer as plain text (`tmux capture-pane -p`).
    pub buffer: &'a str,
}

/// Decides an agent's [`AgentStatus`] from its pane. One implementor per
/// agent kind — deck ships Claude Code today; Codex / pi / opencode each
/// get their own as their TUIs are characterized. Implementations are pure
/// (no IO) so they run cheaply every refresh and are trivial to unit-test.
pub trait StatusClassifier {
    fn classify(&self, pane: &PaneSnapshot) -> AgentStatus;
}

/// The classifier for an agent kind. Kinds without a real classifier fall
/// back to [`UnknownClassifier`] (gray) until one is written.
pub fn classifier_for(kind: AgentKind) -> &'static dyn StatusClassifier {
    static CLAUDE: ClaudeClassifier = ClaudeClassifier;
    static UNKNOWN: UnknownClassifier = UnknownClassifier;
    match kind {
        AgentKind::Claude => &CLAUDE,
        // TODO: a CodexClassifier once its TUI states are characterized.
        AgentKind::Codex => &UNKNOWN,
    }
}

/// Convenience: classify a `kind`'s status from a raw buffer string.
pub fn classify_status(kind: AgentKind, buffer: &str) -> AgentStatus {
    classifier_for(kind).classify(&PaneSnapshot { buffer })
}

/// Fallback classifier for agent kinds not yet characterized: always
/// `Unknown` (gray dot).
pub struct UnknownClassifier;

impl StatusClassifier for UnknownClassifier {
    fn classify(&self, _pane: &PaneSnapshot) -> AgentStatus {
        AgentStatus::Unknown
    }
}

/// Claude Code's pane is classified by reading the **bottom slice** of the
/// capture **bottom-up**: the lowest status-bearing line reflects the current
/// state (lines above it are stale transcript). Per line:
/// - an in-flight turn → `Working` (green), detected by verb-independent
///   tells so it survives Claude's rotating spinner verbs (see
///   [`CLAUDE_INTERRUPT_HINT`] / [`working_timer_tail`] / [`working_spinner_glyph`]
///   / [`working_spinner_tail`] / [`working_tool_tail`]);
/// - a permission/confirmation dialog ("Do you want to proceed?") →
///   `Waiting` (yellow), the user's input is needed;
/// - a finished-turn summary "…ed for <number>…" (e.g. "Cogitated for 5s")
///   → `Idle` (red), the task is done;
/// - nothing recognized → idle at the prompt; an empty capture → `Unknown`.
pub struct ClaudeClassifier;

/// How many lines up from the bottom to consider at all — the capture is
/// roughly one screen, but cap it so a busy transcript can't sway the verdict.
const MAX_SCAN_LINES: usize = 40;

/// A live spinner / tool line sits near the bottom of the pane, a few lines
/// above the input box. Tells that could otherwise match completed transcript
/// (the bare-spinner / bare-tool tiers) are gated to this many bottom lines —
/// counting only **non-blank** lines, so the blank rows around the input box
/// don't push the spinner out of the window.
const LIVE_TAIL_LINES: usize = 12;

/// The phrase Claude Code prints only while a turn is interruptible (in
/// flight): "… · esc to interrupt)". The single most reliable "working" tell
/// — verb-, glyph-, and description-independent, and it does not survive in
/// the transcript once the turn completes, so it can be matched anywhere in
/// the scanned tail without picking up stale lines.
const CLAUDE_INTERRUPT_HINT: &str = "esc to interrupt";

/// Spinner progress tail carrying a live timer: "… (5m 21s", "… (30s)". A
/// secondary tell for spinner lines whose interrupt hint is truncated or
/// wrapped off. Requiring a leading *duration* after "(" rejects completed
/// tool lines like "Reading 1 file… (ctrl+o to expand)" and stray prose
/// ("… (see above)").
fn working_timer_tail() -> &'static regex::Regex {
    static R: OnceLock<regex::Regex> = OnceLock::new();
    R.get_or_init(|| regex::Regex::new(r"(?:\u{2026}|\.\.\.)\s*\(\s*\d+\s*[hms]").unwrap())
}

/// A line led by one of Claude Code's animated spinner glyphs — the rotating
/// sparkle/star frames (NOT the ambiguous "* · •", which double as markdown
/// bullets and are left to the gated gerund/timer tells). The spinner glyph is
/// shown while a turn runs whatever follows it — "✻ Waiting for 1 dynamic
/// workflow to finish" — so the leading glyph alone signals work. The caller
/// gates it to the bottom [`LIVE_TAIL_LINES`] non-blank lines (a real spinner
/// sits a few lines above the input box) and runs it only after the
/// completed-turn check, since the finished summary ("✶ Cogitated for 8s")
/// reuses the same glyph.
fn working_spinner_glyph() -> &'static regex::Regex {
    static R: OnceLock<regex::Regex> = OnceLock::new();
    R.get_or_init(|| {
        regex::Regex::new(
            r"^\s*[\u{2722}\u{2726}\u{2727}\u{2731}\u{2733}\u{2734}\u{2735}\u{2736}\u{2737}\u{2738}\u{2739}\u{273a}\u{273b}\u{273d}\u{2749}\u{274b}]",
        )
        .unwrap()
    })
}

/// A bare thinking spinner whose glyph isn't in [`working_spinner_glyph`]'s
/// set, keyed on structure instead: a leading symbol and a gerund "…ing" right
/// before the trailing ellipsis (e.g. "⟳ Frobnicating…"). Also caller-gated to
/// the bottom [`LIVE_TAIL_LINES`].
fn working_spinner_tail() -> &'static regex::Regex {
    static R: OnceLock<regex::Regex> = OnceLock::new();
    R.get_or_init(|| regex::Regex::new(r"(?i)^\s*[^\w\s].*ing(?:\u{2026}|\.\.\.)\s*$").unwrap())
}

/// A tool call in flight with no parenthetical, e.g. "Reading 1 file…",
/// "Running command…". Tool verbs are a stable, finite set (unlike the
/// thinking spinner's whimsical verbs), so a whitelist is safe here and wards
/// off matching stray prose. Requires the live trailing ellipsis a completed
/// tool line lacks; the caller gates it to the bottom [`LIVE_TAIL_LINES`].
fn working_tool_tail() -> &'static regex::Regex {
    static R: OnceLock<regex::Regex> = OnceLock::new();
    R.get_or_init(|| {
        regex::Regex::new(
            r"(?i)^\s*(?:reading|writing|editing|updating|running|executing|searching|grepping|fetching|building|testing|installing|committing|pushing|analyzing|checking)\b.*(?:\u{2026}|\.\.\.)\s*$",
        )
        .unwrap()
    })
}

/// Markers of a permission/confirmation dialog awaiting the user's choice.
const CLAUDE_WAITING_MARKERS: &[&str] = &["do you want", "\u{276f} 1."];

/// A finished-turn summary line: "…ed for <number>…" (the past-tense verb
/// Claude Code prints when a turn completes, e.g. "Cogitated for 8s").
fn completed_line(lower: &str) -> bool {
    lower.match_indices("ed for ").any(|(i, m)| {
        lower[i + m.len()..]
            .trim_start()
            .starts_with(|c: char| c.is_ascii_digit())
    })
}

impl StatusClassifier for ClaudeClassifier {
    fn classify(&self, pane: &PaneSnapshot) -> AgentStatus {
        if pane.buffer.trim().is_empty() {
            return AgentStatus::Unknown;
        }
        let lines: Vec<&str> = pane.buffer.lines().collect();
        let scanned = &lines[lines.len().saturating_sub(MAX_SCAN_LINES)..];
        // Count non-blank lines from the bottom so the blank rows around the
        // input box don't shrink the live-tail window.
        let mut content_seen = 0usize;
        for line in scanned.iter().rev() {
            let lower = line.to_ascii_lowercase();
            // Strong, high-precision tells — matchable anywhere in the tail.
            if lower.contains(CLAUDE_INTERRUPT_HINT) || working_timer_tail().is_match(line) {
                return AgentStatus::Working;
            }
            if CLAUDE_WAITING_MARKERS.iter().any(|m| lower.contains(m)) {
                return AgentStatus::Waiting;
            }
            // A finished-turn summary ("✶ Cogitated for 8s") reuses a spinner
            // glyph, so rule it out before the bare-spinner tells below.
            if completed_line(&lower) {
                return AgentStatus::Idle;
            }
            if !line.trim().is_empty() {
                content_seen += 1;
            }
            // Bare in-flight tells (no parenthetical), only within the bottom
            // LIVE_TAIL_LINES non-blank lines where a live spinner / tool line
            // sits.
            if content_seen <= LIVE_TAIL_LINES
                && (working_spinner_glyph().is_match(line)
                    || working_spinner_tail().is_match(line)
                    || working_tool_tail().is_match(line))
            {
                return AgentStatus::Working;
            }
        }
        // No status line recognized → sitting at the prompt.
        AgentStatus::Idle
    }
}

/// Classify a process by its `argv` string, returning the agent kind only
/// for an *interactive* invocation. Headless / non-interactive forms
/// return `None` (see `docs/agent-integration.md`).
fn classify(args: &str) -> Option<AgentKind> {
    let tokens: Vec<&str> = args.split_whitespace().collect();
    let arg0 = *tokens.first()?;
    let base = arg0.rsplit('/').next().unwrap_or(arg0);
    match base {
        "claude" => {
            let headless = args.contains("stream-json")
                || arg0.contains("/native-binary/claude")
                || tokens.iter().any(|t| *t == "-p" || *t == "--print");
            (!headless).then_some(AgentKind::Claude)
        }
        "codex" => {
            // Interactive by default — bare `codex`, `codex [PROMPT]`,
            // `resume`/`fork`. Only a known non-interactive subcommand (the
            // first non-flag token) disqualifies it; defaulting to
            // interactive avoids misreading a prompt or a flag value as a
            // subcommand.
            let sub = tokens.iter().skip(1).find(|t| !t.starts_with('-')).copied();
            let non_interactive = matches!(
                sub,
                Some(
                    "exec"
                        | "e"
                        | "review"
                        | "login"
                        | "logout"
                        | "mcp"
                        | "mcp-server"
                        | "app-server"
                        | "remote-control"
                        | "app"
                        | "completion"
                        | "update"
                        | "doctor"
                        | "sandbox"
                        | "debug"
                        | "apply"
                        | "a"
                        | "plugin"
                        | "cloud"
                )
            );
            (!non_interactive).then_some(AgentKind::Codex)
        }
        _ => None,
    }
}

/// Locate the interactive agent (if any) in each pane.
///
/// `ps_output` is `ps -axo pid=,ppid=,args=` (local, or via `ssh <host>
/// ps …`). For each pane we take the *shallowest* matching agent
/// (breadth-first from the pane pid), so a parent agent's sub-agent
/// children don't double-count and a pane yields at most one agent.
/// Agent processes not under any pane (e.g. IDE-extension headless
/// instances) are never reached, so they're excluded.
pub fn detect_agents(panes: &[PaneInfo], ps_output: &str) -> Vec<DetectedAgent> {
    let mut args_of: HashMap<u32, String> = HashMap::new();
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for line in ps_output.lines() {
        let mut it = line.split_whitespace();
        let (Some(pid), Some(ppid)) = (
            it.next().and_then(|s| s.parse::<u32>().ok()),
            it.next().and_then(|s| s.parse::<u32>().ok()),
        ) else {
            continue;
        };
        // Whitespace-normalized args — fine, `classify` re-tokenizes.
        let args = it.collect::<Vec<_>>().join(" ");
        children.entry(ppid).or_default().push(pid);
        args_of.insert(pid, args);
    }

    let mut found = Vec::new();
    for p in panes {
        let mut queue: VecDeque<u32> = VecDeque::new();
        queue.push_back(p.pid);
        while let Some(pid) = queue.pop_front() {
            if let Some(kind) = args_of.get(&pid).and_then(|a| classify(a)) {
                found.push(DetectedAgent {
                    kind,
                    session: p.session.clone(),
                    window: p.window.clone(),
                    pane: p.pane.clone(),
                    pane_id: p.pane_id.clone(),
                    // No buffer here; the gathering layer captures the pane
                    // and fills this in via `classify_status`.
                    status: AgentStatus::Unknown,
                });
                break; // one agent per pane
            }
            if let Some(kids) = children.get(&pid) {
                queue.extend(kids);
            }
        }
    }
    found
}

/// Snapshot of the process table for agent detection: `ps -axo
/// pid=,ppid=,args=`. Empty string on failure (→ no agents).
///
/// Runs through the bounded `CommandRunner` — this is called from the
/// single refresh worker thread, where an unbounded spawn that wedges
/// would freeze the whole status pipeline (see `infra::command`).
pub fn ps_snapshot() -> String {
    // A full process-table dump can be slower than a tmux IPC call on a
    // busy box; give it more headroom than `TMUX_TIMEOUT` while still
    // rescuing the worker from a hang.
    const PS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
    crate::infra::command::default_runner()
        .run("ps", &["-axo", "pid=,ppid=,args="], PS_TIMEOUT)
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "../../tests/unit/infra/agent.rs"]
mod tests;
