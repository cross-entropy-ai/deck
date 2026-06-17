//! Detect interactive coding agents (Claude Code, Codex) from pane process
//! trees, and classify their visible terminal-buffer status.
//!
//! This crate is the pure, IO-free core of deck's agent detection: given a
//! list of tmux panes ([`PaneInfo`]) and a process-table dump (`ps -axo
//! pid=,ppid=,args=`), [`detect_agents`] walks each pane's subtree and returns
//! the interactive agent (if any) running in it. [`classify_status`] reads an
//! agent's raw pane buffer and derives a traffic-light [`AgentStatus`].
//!
//! Detection targets *interactive* invocations only: `pane_current_command`
//! is unreliable (it shows Claude Code's version string and flips while a
//! subprocess runs), so the whole pane subtree is searched and the agent's
//! `argv` is matched. Headless / non-interactive forms are excluded. See
//! deck's `docs/agent-integration.md` for signature research.
//!
//! The crate depends only on `regex` and carries no tmux, ssh, or deck-specific
//! state; runtime collection (running `ps`, capturing panes, timeouts) lives in
//! deck.

use std::collections::{HashMap, VecDeque};
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentKind {
    Claude,
    Codex,
}

/// One tmux pane's identity, fed to `detect_agents`. `session`/`window`/
/// `pane` are display fields (`window` is the window *name*); `pane_id` is
/// the stable `%N` switch handle — names and indices both churn as
/// panes/windows change, so only `pane_id` is a safe target.
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
    /// Traffic-light health from the pane buffer (see [`classify_status`]).
    /// `detect_agents` has no buffer, so it leaves this `Unknown` for the
    /// gathering layer to fill in.
    pub status: AgentStatus,
}

/// Traffic-light health shown as a colored dot per agent row: green =
/// working, red = idle, yellow = waiting for input, default = unknown (not
/// captured, or no classifier yet). The renderer
/// (`ui::sidebar::sessions::recolor_agent_dot`) maps color off this status,
/// not the glyph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AgentStatus {
    Working,
    Idle,
    Waiting,
    #[default]
    Unknown,
}

impl DetectedAgent {
    /// Compact `session:window` location for display. The pane index is
    /// omitted — it's noise in the sidebar; the real tmux target is `pane_id`.
    pub fn location(&self) -> String {
        format!("{}:{}", self.session, self.window)
    }
}

/// Classify a `kind`'s status from its raw pane buffer. Pure (no IO) so it
/// runs cheaply every refresh and is trivial to unit-test. Kinds without a
/// real classifier stay `Unknown` (gray dot) until one is written.
pub fn classify_status(kind: AgentKind, buffer: &str) -> AgentStatus {
    match kind {
        AgentKind::Claude => claude_classify(buffer),
        // TODO: characterize Codex's TUI states.
        AgentKind::Codex => AgentStatus::Unknown,
    }
}

/// How many lines up from the bottom to consider at all — the capture is
/// roughly one screen, but cap it so a busy transcript can't sway the verdict.
const MAX_SCAN_LINES: usize = 40;

/// A live spinner/tool line sits a few lines above the input box. The
/// bare-spinner/bare-tool tells (which could match completed transcript) are
/// gated to this many bottom **non-blank** lines, so blank rows around the
/// input box don't push the spinner out of the window.
const LIVE_TAIL_LINES: usize = 12;

/// The phrase Claude Code prints only while a turn is interruptible:
/// "… · esc to interrupt)". Most reliable "working" tell (verb-, glyph-,
/// description-independent) and gone from the transcript once the turn ends,
/// so it can match anywhere in the scanned tail without hitting stale lines.
const CLAUDE_INTERRUPT_HINT: &str = "esc to interrupt";

/// Spinner progress tail with a live timer: "… (5m 21s", "… (30s)". Backup
/// tell for spinner lines whose interrupt hint is truncated/wrapped off.
/// Requiring a *duration* right after "(" rejects completed tool lines
/// ("Reading 1 file… (ctrl+o to expand)") and stray prose ("… (see above)").
fn working_timer_tail() -> &'static regex::Regex {
    static R: OnceLock<regex::Regex> = OnceLock::new();
    R.get_or_init(|| regex::Regex::new(r"(?:\u{2026}|\.\.\.)\s*\(\s*\d+\s*[hms]").unwrap())
}

/// A line led by a Claude Code animated spinner glyph (rotating sparkle/star
/// frames; NOT "* · •", which double as markdown bullets — left to the gated
/// gerund/timer tells). The glyph shows while a turn runs ("✻ Waiting for 1
/// dynamic workflow to finish"), so the leading glyph alone signals work.
/// Caller gates it to the bottom [`LIVE_TAIL_LINES`] non-blank lines and runs
/// it only after the completed-turn check, since the finished summary
/// ("✶ Cogitated for 8s") reuses the same glyph.
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
/// set, keyed on structure: leading symbol + gerund "…ing" before the trailing
/// ellipsis ("⟳ Frobnicating…"). Caller-gated to the bottom [`LIVE_TAIL_LINES`].
fn working_spinner_tail() -> &'static regex::Regex {
    static R: OnceLock<regex::Regex> = OnceLock::new();
    R.get_or_init(|| regex::Regex::new(r"(?i)^\s*[^\w\s].*ing(?:\u{2026}|\.\.\.)\s*$").unwrap())
}

/// A tool call in flight with no parenthetical ("Reading 1 file…", "Running
/// command…"). Tool verbs are a stable finite set, so a whitelist is safe and
/// avoids matching stray prose. Requires the live trailing ellipsis a completed
/// tool line lacks; caller gates it to the bottom [`LIVE_TAIL_LINES`].
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

/// Classify Claude Code's pane by scanning the capture's bottom slice
/// bottom-up; the lowest status-bearing line wins (lines above are stale
/// transcript). Per line:
/// - in-flight turn → `Working`, via verb-independent tells that survive
///   Claude's rotating spinner verbs ([`CLAUDE_INTERRUPT_HINT`],
///   [`working_timer_tail`], [`working_spinner_glyph`],
///   [`working_spinner_tail`], [`working_tool_tail`]);
/// - permission/confirmation dialog ("Do you want to proceed?") → `Waiting`;
/// - finished-turn summary "…ed for <number>…" ("Cogitated for 5s") → `Idle`;
/// - nothing recognized → `Idle` at prompt; empty capture → `Unknown`.
fn claude_classify(buffer: &str) -> AgentStatus {
    if buffer.trim().is_empty() {
        return AgentStatus::Unknown;
    }
    let lines: Vec<&str> = buffer.lines().collect();
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
            // Interactive by default (bare `codex`, `codex [PROMPT]`,
            // `resume`/`fork`). Only a known non-interactive subcommand (first
            // non-flag token) disqualifies it; defaulting to interactive avoids
            // misreading a prompt or flag value as a subcommand.
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
/// `ps_output` is `ps -axo pid=,ppid=,args=` (local or via ssh). Per pane,
/// take the *shallowest* matching agent (breadth-first from the pane pid), so
/// sub-agent children don't double-count and a pane yields at most one agent.
/// Processes under no pane (e.g. IDE-extension headless instances) are never
/// reached, so they're excluded.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_claude_interactive_vs_headless() {
        // Interactive forms.
        assert_eq!(classify("claude"), Some(AgentKind::Claude));
        assert_eq!(
            classify("claude --dangerously-skip-permissions"),
            Some(AgentKind::Claude)
        );
        assert_eq!(
            classify("/Users/me/.local/bin/claude --resume abc"),
            Some(AgentKind::Claude)
        );
        // Headless forms are not counted.
        assert_eq!(classify("claude -p hello"), None);
        assert_eq!(classify("claude --print hi"), None);
        assert_eq!(
            classify("claude --output-format stream-json --verbose"),
            None
        );
        assert_eq!(
            classify("/Users/me/.cursor/extensions/x/resources/native-binary/claude --output-format stream-json"),
            None
        );
    }

    #[test]
    fn classify_codex_interactive_vs_subcommands() {
        // Interactive: bare, with a prompt, resume, fork — incl. the native
        // binary path the node wrapper spawns.
        assert_eq!(classify("codex"), Some(AgentKind::Codex));
        assert_eq!(classify("codex \"fix the bug\""), Some(AgentKind::Codex));
        assert_eq!(classify("codex resume"), Some(AgentKind::Codex));
        assert_eq!(classify("codex --model o3 fork"), Some(AgentKind::Codex));
        assert_eq!(
            classify(
                "/Users/me/.bun/.../@openai/codex-darwin-arm64/vendor/aarch64-apple-darwin/codex/codex"
            ),
            Some(AgentKind::Codex)
        );
        // Non-interactive subcommands are not counted.
        for sub in [
            "exec",
            "review",
            "mcp",
            "mcp-server",
            "app-server",
            "remote-control",
            "cloud",
            "login",
        ] {
            assert_eq!(
                classify(&format!("codex {sub} --flag")),
                None,
                "codex {sub}"
            );
        }
    }

    #[test]
    fn classify_ignores_non_agents() {
        assert_eq!(classify("-zsh"), None);
        assert_eq!(classify("/bin/zsh"), None);
        assert_eq!(classify("node /path/to/vite"), None);
        assert_eq!(classify("vim"), None);
        assert_eq!(classify(""), None);
    }

    fn pane(pid: u32, session: &str, window: &str, pane: &str) -> PaneInfo {
        PaneInfo {
            pid,
            session: session.to_string(),
            window: window.to_string(),
            pane: pane.to_string(),
            pane_id: format!("%{pid}"),
        }
    }

    #[test]
    fn detect_agents_one_per_pane_excludes_subagents_and_headless() {
        // pane 100: shell -> claude -> (sub-agent claude child, must NOT double-count)
        // pane 300: shell -> node wrapper -> native codex (matched at depth 2)
        // pane 500: shell -> vim (no agent)
        // pid 700: a headless claude NOT under any pane (ppid 1) -> excluded
        let ps = "\
100 1 -zsh
200 100 claude --dangerously-skip-permissions
250 200 claude --dangerously-skip-permissions
300 1 -zsh
400 300 node /Users/me/.bun/bin/codex
410 400 /Users/me/.bun/vendor/codex
500 1 -zsh
600 500 vim
700 1 /Users/me/.cursor/native-binary/claude --output-format stream-json";
        let panes = [
            pane(100, "deck", "main", "0"),
            pane(300, "work", "agents", "1"),
            pane(500, "work", "agents", "2"),
        ];
        let agents = detect_agents(&panes, ps);
        assert_eq!(agents.len(), 2);
        // pane 100 -> claude, located at its session/window-name/pane, with the
        // stable pane id carried for switching.
        assert_eq!(agents[0].kind, AgentKind::Claude);
        assert_eq!(agents[0].location(), "deck:main");
        assert_eq!(agents[0].pane_id, "%100");
        // pane 300 -> codex (matched two levels down the wrapper).
        assert_eq!(agents[1].kind, AgentKind::Codex);
        assert_eq!(agents[1].location(), "work:agents");
        assert_eq!(agents[1].pane_id, "%300");
        // pane 500 -> no agent (not in the list).
    }

    #[test]
    fn detect_agents_run_as_pane_root() {
        // Some setups exec the agent as the pane's command (pane_pid IS the
        // agent), with no intervening shell.
        let ps = "800 1 claude\n900 1 -zsh";
        let agents = detect_agents(&[pane(800, "s", "0", "0"), pane(900, "s", "1", "0")], ps);
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].kind, AgentKind::Claude);
    }

    #[test]
    fn detect_agents_empty_inputs() {
        assert!(detect_agents(&[], "").is_empty());
        assert!(detect_agents(&[pane(1, "s", "0", "0")], "").is_empty());
    }

    #[test]
    fn claude_classifier_reads_traffic_light_from_buffer() {
        // Working spinner: the "<verb>ing… (… esc to interrupt)" status line.
        let working = "✶ Cogitating… (12s · ↑ 3.2k tokens · esc to interrupt)";
        assert_eq!(
            classify_status(AgentKind::Claude, working),
            AgentStatus::Working
        );

        // Working spinner with a description between the verb and the tail — the
        // old "ing… (" marker missed this; the interrupt hint catches it.
        let described = "✱ Distilling findings into ai-patterns.md… (5m 21s · esc to interrupt)";
        assert_eq!(
            classify_status(AgentKind::Claude, described),
            AgentStatus::Working
        );

        // Working spinner whose interrupt hint is wrapped off — the live timer
        // tail "… (30s" still reads as working.
        let timer = "* Brewing… (30s · ↑ 1.2k tokens";
        assert_eq!(
            classify_status(AgentKind::Claude, timer),
            AgentStatus::Working
        );

        // Same, with ASCII "..." instead of the "…" glyph and the hint truncated.
        let ascii_dots = "* Distilling findings into ai-patterns.md... (5m 21s   17.6k toke";
        assert_eq!(
            classify_status(AgentKind::Claude, ascii_dots),
            AgentStatus::Working
        );

        // A bare thinking spinner (glyph + gerund + ellipsis, no parenthetical),
        // e.g. just after a turn starts before the timer renders.
        let spinner = "· Crunching…";
        assert_eq!(
            classify_status(AgentKind::Claude, spinner),
            AgentStatus::Working
        );

        // A spinner glyph leading a non-gerund, non-ellipsis status line — the
        // glyph alone signals an in-flight turn.
        let workflow = "✻ Waiting for 1 dynamic workflow to finish";
        assert_eq!(
            classify_status(AgentKind::Claude, workflow),
            AgentStatus::Working
        );

        // Realistic layout: the spinner sits a few lines above the input box, with
        // blank rows in between. Blank rows don't count toward the live-tail
        // window, so the glyph is still seen.
        let with_box = "✻ Waiting for 1 dynamic workflow to finish\n\n╭──────────────────────╮\n│ >                    │\n╰──────────────────────╯\n\n  ? for shortcuts";
        assert_eq!(
            classify_status(AgentKind::Claude, with_box),
            AgentStatus::Working
        );

        // A spinner glyph beyond the live-tail window (too far above the bottom)
        // is stale transcript, not the current state → idle.
        let stale_spinner = format!("✻ Cogitating\n{}\n│ >", "x\n".repeat(14));
        assert_eq!(
            classify_status(AgentKind::Claude, &stale_spinner),
            AgentStatus::Idle
        );

        // A bare tool line in flight (no parenthetical) on the bottom line.
        let tool = "some earlier output\nRunning command…";
        assert_eq!(
            classify_status(AgentKind::Claude, tool),
            AgentStatus::Working
        );

        // A "… (ctrl+o to expand)" tool-result tail is NOT a live timer → not
        // working on its own (no interrupt hint, paren isn't a duration).
        let collapsed =
            "⏺ Read(config.yaml)\n  ⎿ Read 1 file… (ctrl+o to expand)\n│ > │\n? for shortcuts";
        assert_eq!(
            classify_status(AgentKind::Claude, collapsed),
            AgentStatus::Idle
        );

        // Idle at the prompt, nothing pending.
        let idle = "╭───────────╮\n│ > │\n╰───────────╯\n? for shortcuts";
        assert_eq!(classify_status(AgentKind::Claude, idle), AgentStatus::Idle);

        // A permission dialog → waiting on the user.
        let prompt = "Do you want to proceed?\n❯ 1. Yes\n  2. No";
        assert_eq!(
            classify_status(AgentKind::Claude, prompt),
            AgentStatus::Waiting
        );

        // A finished turn ("…ed for <n>") below a stale spinner reads as idle —
        // bottom-up wins.
        let done = "✶ Cogitating… (3s · esc to interrupt)\n● Done\n✶ Cogitated for 8s";
        assert_eq!(classify_status(AgentKind::Claude, done), AgentStatus::Idle);

        // Empty capture → unknown.
        assert_eq!(
            classify_status(AgentKind::Claude, "   "),
            AgentStatus::Unknown
        );
    }
}
