//! Detect coding agents (Claude Code, Codex) running *interactively* in
//! tmux panes, with no hooks installed — purely by walking the process
//! tree under each pane's pid and matching the agent's `argv`.
//!
//! See `docs/agent-integration.md` for the research behind the
//! signatures. In short: `pane_current_command` is unreliable (it shows
//! Claude Code's version string, and flips while the agent runs a
//! subprocess), so we instead enumerate pane pids and look for an agent
//! process anywhere in each pane's subtree.

use std::collections::{HashMap, VecDeque};

/// How many interactive agents of each kind were detected in a scope —
/// the local tmux server, or one remote host.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AgentCounts {
    pub claude: usize,
    pub codex: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentKind {
    Claude,
    Codex,
}

/// Classify a process by its `argv` string, returning the agent kind
/// only for an *interactive* invocation. Headless / non-interactive
/// forms return `None` so they aren't counted:
/// - Claude Code: `-p`/`--print`/`--output-format stream-json`, and the
///   IDE-extension `.../native-binary/claude … stream-json`.
/// - Codex: any subcommand other than `resume`/`fork` (`exec`, `review`,
///   `mcp`, `app-server`, `remote-control`, `cloud`, …).
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
            // `resume`/`fork`. Only a known *non-interactive* subcommand
            // (the first non-flag token) disqualifies it. Defaulting to
            // interactive avoids misreading a prompt or a flag value
            // (e.g. `--model o3`) as a subcommand.
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

/// Count panes running an interactive agent.
///
/// - `pane_pids`: the tmux pane root pids (`#{pane_pid}`).
/// - `ps_output`: output of `ps -axo pid=,ppid=,args=` (run locally, or
///   via `ssh <host> ps …` for a remote host).
///
/// For each pane we take the *shallowest* matching agent (breadth-first
/// from the pane pid), so a parent agent's sub-agent children don't
/// inflate the count, and a pane is counted at most once. Agent
/// processes that aren't under any tmux pane (e.g. IDE-extension
/// headless instances) are never reached, so they're naturally excluded.
pub fn count_agents(pane_pids: &[u32], ps_output: &str) -> AgentCounts {
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

    let mut counts = AgentCounts::default();
    for &root in pane_pids {
        let mut queue: VecDeque<u32> = VecDeque::new();
        queue.push_back(root);
        while let Some(pid) = queue.pop_front() {
            if let Some(kind) = args_of.get(&pid).and_then(|a| classify(a)) {
                match kind {
                    AgentKind::Claude => counts.claude += 1,
                    AgentKind::Codex => counts.codex += 1,
                }
                break; // one agent per pane
            }
            if let Some(kids) = children.get(&pid) {
                queue.extend(kids);
            }
        }
    }
    counts
}

/// Snapshot of the process table for agent detection: `ps -axo
/// pid=,ppid=,args=`. Empty string on failure (→ zero counts).
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
