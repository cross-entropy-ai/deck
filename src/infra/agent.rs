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
/// red = actively working, green = idle, yellow = waiting for user input,
/// gray = unknown (not captured, or an agent kind whose classifier isn't
/// implemented yet). Color mapping lives in the renderer; these stay
/// semantic.
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

/// Claude Code's pane is classified by reading it **bottom-up**: the lowest
/// status-bearing line reflects the current state (lines above it are stale
/// transcript). Per line:
/// - a working spinner like "Cogitating… (12s · esc to interrupt)" — tell
///   is `ing… (` → `Working` (red);
/// - a permission/confirmation dialog ("Do you want to proceed?") →
///   `Waiting` (yellow), the user's input is needed;
/// - a finished-turn summary "…ed for <number>…" (e.g. "Cogitated for 5s")
///   → `Idle` (green), the task is done;
/// - nothing recognized → idle at the prompt; an empty capture → `Unknown`.
pub struct ClaudeClassifier;

/// Substring that marks an in-flight turn: Claude Code's "<verb>ing… ("
/// status line (e.g. "Cogitating… (3s · …").
const CLAUDE_WORKING_MARKER: &str = "ing\u{2026} (";

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
        for line in pane.buffer.lines().rev() {
            let lower = line.to_ascii_lowercase();
            if lower.contains(CLAUDE_WORKING_MARKER) {
                return AgentStatus::Working;
            }
            if CLAUDE_WAITING_MARKERS.iter().any(|m| lower.contains(m)) {
                return AgentStatus::Waiting;
            }
            if completed_line(&lower) {
                return AgentStatus::Idle;
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

/// Parse `tmux list-panes -F '#{pane_pid}\t#{session_name}\t#{window_index}\t#{pane_index}'`
/// output into `PaneInfo`s. Shared by the local and ssh gathering paths.
pub fn parse_panes(raw: &str) -> Vec<PaneInfo> {
    raw.lines()
        .filter_map(|line| {
            let mut f = line.split('\t');
            let pid = f.next()?.trim().parse::<u32>().ok()?;
            Some(PaneInfo {
                pid,
                session: f.next()?.to_string(),
                window: f.next()?.to_string(),
                pane: f.next()?.to_string(),
                pane_id: f.next()?.to_string(),
            })
        })
        .collect()
}

/// The `-F` format string for the pane fields `parse_panes` expects.
pub const PANE_FORMAT: &str =
    "#{pane_pid}\t#{session_name}\t#{window_index}\t#{pane_index}\t#{pane_id}";

/// Snapshot of the process table for agent detection: `ps -axo
/// pid=,ppid=,args=`. Empty string on failure (→ no agents).
pub fn ps_snapshot() -> String {
    std::process::Command::new("ps")
        .args(["-axo", "pid=,ppid=,args="])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "../../tests/unit/infra/agent.rs"]
mod tests;
