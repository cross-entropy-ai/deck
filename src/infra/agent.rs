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

impl AgentKind {
    pub fn label(self) -> &'static str {
        match self {
            AgentKind::Claude => "claude",
            AgentKind::Codex => "codex",
        }
    }
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
}

impl DetectedAgent {
    /// Compact `session:window.pane` location for display.
    pub fn location(&self) -> String {
        format!("{}:{}.{}", self.session, self.window, self.pane)
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
                    "exec" | "e" | "review" | "login" | "logout" | "mcp" | "mcp-server"
                        | "app-server" | "remote-control" | "app" | "completion" | "update"
                        | "doctor" | "sandbox" | "debug" | "apply" | "a" | "plugin" | "cloud"
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
