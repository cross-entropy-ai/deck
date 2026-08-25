//! Install, remove, and inspect deck's optional agent lifecycle hooks.
//!
//! The hooks are the third status source of `docs/agent-status-plan.md`: a
//! managed `/bin/sh` script (embedded from `assets/agent-hooks/`) that each
//! agent's own hook mechanism runs on lifecycle events, writing
//! `@deck_agent_state` onto the tmux pane the agent lives in. deck's probes
//! read it back for free (`infra::parser::pane::PANE_FORMAT`).
//!
//! Everything here happens only on an **explicit user action** (`deck hooks
//! install|uninstall|status`) — never as a side effect of attach or refresh.
//! Codex gates changed hooks behind a blocking trust prompt on its next
//! launch; a hook that appears without a user action the user can connect it
//! to reads as an attack, so the timing rule is load-bearing. For the same
//! reason installs are **byte-stable**: rewriting an unchanged `hooks.json`
//! would re-trigger that prompt on every machine, so nothing is written
//! unless content actually changes.
//!
//! Only the user-level config is touched (`~/.claude/settings.json`,
//! `~/.codex/hooks.json`) — never a project-level file that would end up in
//! someone's repository. deck's entries carry a `"_deck": true` marker (the
//! pattern Otty uses) so install/uninstall can find exactly its own entries
//! and leave every user hook alone; the script is a separate managed file
//! beside the config (the pattern herdr uses), overwritten on reinstall.

use serde_json::{json, Map, Value};

use crate::infra::command::default_runner;
use crate::remote_tmux::{run_ssh, shell_single_quote};

/// Version of the managed hook script. Bumping it makes every Codex on every
/// machine re-prompt its hook trust gate on next launch — bump only for a
/// behavior change, never cosmetics, and keep it in sync with the
/// `DECK_HOOK_VERSION=` line inside the script.
pub const DECK_HOOK_VERSION: &str = "1";

const HOOK_SCRIPT: &str = include_str!("../../assets/agent-hooks/deck-agent-state.sh");
const SCRIPT_NAME: &str = "deck-agent-state.sh";

/// The subscription table, identical for both agents (measured in
/// `docs/agent-status-plan.md`): identity on `SessionStart`, the three state
/// edges, and cleanup. Deliberately **no tool events** — the activity clock
/// covers working-ness without taxing every tool call — and no
/// `SubagentStop` (Claude fires it with no subagent involved; the script
/// additionally drops any payload carrying `agent_id`).
const EVENTS: &[(&str, &str, u64)] = &[
    ("SessionStart", "session", 5),
    ("UserPromptSubmit", "working", 5),
    ("PermissionRequest", "blocked", 5),
    ("Stop", "idle", 5),
    // SessionEnd budgets are tight (Codex clamps to 3s, Claude shares a
    // small budget across hooks); one tmux unset fits easily.
    ("SessionEnd", "clear", 3),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookAgent {
    Claude,
    Codex,
}

impl HookAgent {
    pub const ALL: [HookAgent; 2] = [HookAgent::Claude, HookAgent::Codex];

    pub fn label(self) -> &'static str {
        match self {
            HookAgent::Claude => "claude",
            HookAgent::Codex => "codex",
        }
    }

    /// Home-relative config directory.
    fn dir(self) -> &'static str {
        match self {
            HookAgent::Claude => ".claude",
            HookAgent::Codex => ".codex",
        }
    }

    /// Home-relative hooks file. Claude merges hooks into its settings;
    /// Codex keeps a dedicated `hooks.json`.
    fn settings(self) -> &'static str {
        match self {
            HookAgent::Claude => ".claude/settings.json",
            HookAgent::Codex => ".codex/hooks.json",
        }
    }

    fn script(self) -> String {
        format!("{}/{}", self.dir(), SCRIPT_NAME)
    }

    /// The hook command. `$HOME` stays unexpanded so the same command string
    /// is correct on every machine — which also keeps installs byte-stable.
    fn command(self, action: &str) -> String {
        format!("sh \"$HOME/{}\" {action}", self.script())
    }
}

/// What `install`/`uninstall` did for one agent on one target.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Config dir absent — the agent isn't set up on this machine; skipped.
    Absent,
    Installed,
    /// Something was present but stale (older script or entry shape).
    Updated,
    Unchanged,
    Removed,
    NothingToRemove,
    Failed(String),
}

pub struct Report {
    pub agent: HookAgent,
    pub outcome: Outcome,
}

/// Hook state of one agent on one target, for `deck hooks status`.
pub struct HookStatus {
    pub agent: HookAgent,
    /// `None` — no config dir. `Some((script_version, entries_ok))`.
    pub installed: Option<(Option<String>, bool)>,
    /// Codex only: `[features] hooks = false` found in `config.toml`, which
    /// disables hooks entirely — installed entries would never run.
    pub hooks_disabled: bool,
    /// The probe itself failed (host down, ssh error): the other fields say
    /// nothing. An unreachable host must not read as "agent not set up".
    pub error: Option<String>,
}

/// File operations against one target's home directory, so the same install
/// logic runs locally (std::fs) and over ssh (one shell command per op).
/// Paths are home-relative (`.claude/settings.json`).
pub trait HookFs {
    fn dir_exists(&self, rel: &str) -> Result<bool, String>;
    /// `Ok(None)` when the file is absent (or unreadable-as-absent).
    fn read(&self, rel: &str) -> Result<Option<String>, String>;
    fn write(&self, rel: &str, content: &str, executable: bool) -> Result<(), String>;
    fn remove(&self, rel: &str) -> Result<(), String>;
}

pub struct LocalFs;

impl LocalFs {
    fn abs(rel: &str) -> Result<std::path::PathBuf, String> {
        let home = std::env::var("HOME").map_err(|_| "HOME is not set".to_string())?;
        Ok(std::path::Path::new(&home).join(rel))
    }
}

impl HookFs for LocalFs {
    fn dir_exists(&self, rel: &str) -> Result<bool, String> {
        Ok(Self::abs(rel)?.is_dir())
    }

    fn read(&self, rel: &str) -> Result<Option<String>, String> {
        match std::fs::read_to_string(Self::abs(rel)?) {
            Ok(s) => Ok(Some(s)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("read {rel}: {e}")),
        }
    }

    fn write(&self, rel: &str, content: &str, executable: bool) -> Result<(), String> {
        let path = Self::abs(rel)?;
        std::fs::write(&path, content).map_err(|e| format!("write {rel}: {e}"))?;
        #[cfg(unix)]
        if executable {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                .map_err(|e| format!("chmod {rel}: {e}"))?;
        }
        Ok(())
    }

    fn remove(&self, rel: &str) -> Result<(), String> {
        match std::fs::remove_file(Self::abs(rel)?) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("remove {rel}: {e}")),
        }
    }
}

/// One ssh hop per operation, through the same `run_ssh` the probes use (so
/// container ids would get their exec wrapping too, though `deck hooks`
/// currently targets hosts). `$HOME` is left to the remote shell.
pub struct RemoteFs {
    pub host: String,
}

impl RemoteFs {
    fn quoted(rel: &str) -> String {
        // Double quotes: `$HOME` must expand, the rest of the path is ours
        // and contains nothing shell-special.
        format!("\"$HOME/{rel}\"")
    }

    fn run(&self, argv: &[&str]) -> Result<String, String> {
        run_ssh(default_runner(), &self.host, argv).map_err(|e| e.to_string())
    }
}

impl HookFs for RemoteFs {
    fn dir_exists(&self, rel: &str) -> Result<bool, String> {
        let q = Self::quoted(rel);
        let out = self.run(&["test", "-d", &q, "&&", "echo", "yes", "||", "echo", "no"])?;
        Ok(out.trim() == "yes")
    }

    fn read(&self, rel: &str) -> Result<Option<String>, String> {
        let q = Self::quoted(rel);
        let out = self.run(&["cat", &q, "2>/dev/null", "||", "true"])?;
        // An absent file and an empty file both come back empty; callers
        // treat empty settings as absent anyway.
        Ok((!out.trim().is_empty()).then_some(out))
    }

    fn write(&self, rel: &str, content: &str, executable: bool) -> Result<(), String> {
        let q = Self::quoted(rel);
        let quoted = shell_single_quote(content);
        let mode = if executable { "755" } else { "644" };
        let out = self.run(&[
            "printf",
            "%s",
            &quoted,
            ">",
            &q,
            "&&",
            "chmod",
            mode,
            &q,
            "&&",
            "echo",
            "__deck_ok__",
        ])?;
        if out.trim().ends_with("__deck_ok__") {
            Ok(())
        } else {
            Err(format!("write {rel} on {}: unconfirmed", self.host))
        }
    }

    fn remove(&self, rel: &str) -> Result<(), String> {
        let q = Self::quoted(rel);
        self.run(&["rm", "-f", &q]).map(|_| ())
    }
}

/// deck's matcher-group entry for one event. `"_deck": true` is the
/// ownership marker: uninstall and re-install touch exactly the groups that
/// carry it and nothing else.
fn deck_entry(agent: HookAgent, action: &str, timeout: u64) -> Value {
    json!({
        "_deck": true,
        "hooks": [{
            "type": "command",
            "command": agent.command(action),
            "timeout": timeout,
        }]
    })
}

fn is_deck_entry(entry: &Value) -> bool {
    entry.get("_deck").and_then(Value::as_bool) == Some(true)
}

/// Merge deck's hook entries into an agent's hooks JSON. Returns `Ok(None)`
/// when the file already carries exactly the current entries — the
/// byte-stability the Codex trust gate demands — and the new content
/// otherwise. User entries and unknown keys are preserved untouched.
pub fn merged_hooks(agent: HookAgent, existing: &str) -> Result<Option<String>, String> {
    let mut root: Value = if existing.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(existing).map_err(|e| format!("existing hooks unparseable: {e}"))?
    };
    let obj = root
        .as_object_mut()
        .ok_or_else(|| "existing hooks file is not a JSON object".to_string())?;
    let hooks = obj
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| "\"hooks\" is not a JSON object".to_string())?;

    let mut changed = false;
    for (event, action, timeout) in EVENTS {
        let desired = deck_entry(agent, action, *timeout);
        let entries = hooks
            .entry(event.to_string())
            .or_insert_with(|| Value::Array(Vec::new()))
            .as_array_mut()
            .ok_or_else(|| format!("hook entries for {event} are not an array"))?;
        let ours: Vec<&Value> = entries.iter().filter(|e| is_deck_entry(e)).collect();
        // Exactly the current entry already in place → leave the array
        // byte-for-byte alone (position included).
        if ours.len() == 1 && *ours[0] == desired {
            continue;
        }
        entries.retain(|e| !is_deck_entry(e));
        entries.push(desired);
        changed = true;
    }
    if !changed {
        return Ok(None);
    }
    serde_json::to_string_pretty(&root)
        .map(|s| Some(s + "\n"))
        .map_err(|e| e.to_string())
}

/// Remove deck's entries from an agent's hooks JSON. `Ok(None)` when nothing
/// of deck's was present. Events left with no entries lose their key; the
/// rest of the file stays untouched.
pub fn stripped_hooks(existing: &str) -> Result<Option<String>, String> {
    if existing.trim().is_empty() {
        return Ok(None);
    }
    let mut root: Value =
        serde_json::from_str(existing).map_err(|e| format!("existing hooks unparseable: {e}"))?;
    let Some(hooks) = root.get_mut("hooks").and_then(Value::as_object_mut) else {
        return Ok(None);
    };
    let mut changed = false;
    let events: Vec<String> = hooks.keys().cloned().collect();
    for event in events {
        let Some(entries) = hooks.get_mut(&event).and_then(Value::as_array_mut) else {
            continue;
        };
        let before = entries.len();
        entries.retain(|e| !is_deck_entry(e));
        if entries.len() != before {
            changed = true;
            if entries.is_empty() {
                hooks.remove(&event);
            }
        }
    }
    if !changed {
        return Ok(None);
    }
    serde_json::to_string_pretty(&root)
        .map(|s| Some(s + "\n"))
        .map_err(|e| e.to_string())
}

/// Whether a Codex `config.toml` explicitly disables hooks: a top-level
/// `[features]` table containing `hooks = false`. deck reports this rather
/// than flipping it — the setting is the user's, and silently re-enabling a
/// mechanism they turned off is exactly the move the trust gate exists to
/// catch. (Hooks default to on when the key is absent.)
pub fn codex_hooks_disabled(config_toml: &str) -> bool {
    let mut in_features = false;
    for line in config_toml.lines() {
        let line = line.trim();
        if let Some(header) = line.strip_prefix('[') {
            in_features = header.trim_end_matches(']').trim() == "features";
            continue;
        }
        if in_features {
            let mut parts = line.splitn(2, '=');
            if parts.next().map(str::trim) == Some("hooks")
                && parts
                    .next()
                    .map(str::trim)
                    .map(|v| v.split('#').next().unwrap_or_default().trim() == "false")
                    == Some(true)
            {
                return true;
            }
        }
    }
    false
}

/// Extract the `DECK_HOOK_VERSION=` line from an installed script.
fn script_version(script: &str) -> Option<String> {
    script
        .lines()
        .find_map(|l| l.trim().strip_prefix("# DECK_HOOK_VERSION="))
        .map(|v| v.trim().to_string())
}

/// Install both agents' hooks on one target. Per agent: skip when the config
/// dir is absent (the agent isn't set up there), write the managed script
/// only when its content differs, merge the entries only when they differ.
pub fn install(fs: &dyn HookFs) -> Vec<Report> {
    HookAgent::ALL
        .iter()
        .map(|&agent| Report {
            agent,
            outcome: install_one(fs, agent),
        })
        .collect()
}

fn install_one(fs: &dyn HookFs, agent: HookAgent) -> Outcome {
    match fs.dir_exists(agent.dir()) {
        Ok(true) => {}
        Ok(false) => return Outcome::Absent,
        Err(e) => return Outcome::Failed(e),
    }
    let script_rel = agent.script();
    let existing_script = match fs.read(&script_rel) {
        Ok(s) => s,
        Err(e) => return Outcome::Failed(e),
    };
    // Compare modulo trailing whitespace: the ssh read path comes back
    // through `stdout_trimmed`, so an exact compare against the (newline-
    // terminated) embedded script would rewrite the file on every remote
    // reinstall and never report Unchanged.
    let script_changed =
        existing_script.as_deref().map(str::trim_end) != Some(HOOK_SCRIPT.trim_end());
    if script_changed {
        if let Err(e) = fs.write(&script_rel, HOOK_SCRIPT, true) {
            return Outcome::Failed(e);
        }
    }
    let existing = match fs.read(agent.settings()) {
        Ok(s) => s.unwrap_or_default(),
        Err(e) => return Outcome::Failed(e),
    };
    let entries_changed = match merged_hooks(agent, &existing) {
        Ok(Some(merged)) => {
            if let Err(e) = fs.write(agent.settings(), &merged, false) {
                return Outcome::Failed(e);
            }
            true
        }
        Ok(None) => false,
        Err(e) => return Outcome::Failed(e),
    };
    match (existing_script.is_none(), script_changed || entries_changed) {
        (true, _) => Outcome::Installed,
        (false, true) => Outcome::Updated,
        (false, false) => Outcome::Unchanged,
    }
}

/// Remove deck's hooks from one target: its entries out of the hooks file,
/// then the managed script. Everything else stays (herdr's uninstall
/// likewise leaves the rest of the config alone).
pub fn uninstall(fs: &dyn HookFs) -> Vec<Report> {
    HookAgent::ALL
        .iter()
        .map(|&agent| Report {
            agent,
            outcome: uninstall_one(fs, agent),
        })
        .collect()
}

fn uninstall_one(fs: &dyn HookFs, agent: HookAgent) -> Outcome {
    match fs.dir_exists(agent.dir()) {
        Ok(true) => {}
        Ok(false) => return Outcome::Absent,
        Err(e) => return Outcome::Failed(e),
    }
    let mut removed = false;
    match fs.read(agent.settings()) {
        Ok(Some(existing)) => match stripped_hooks(&existing) {
            Ok(Some(stripped)) => {
                if let Err(e) = fs.write(agent.settings(), &stripped, false) {
                    return Outcome::Failed(e);
                }
                removed = true;
            }
            Ok(None) => {}
            Err(e) => return Outcome::Failed(e),
        },
        Ok(None) => {}
        Err(e) => return Outcome::Failed(e),
    }
    match fs.read(&agent.script()) {
        Ok(Some(_)) => {
            if let Err(e) = fs.remove(&agent.script()) {
                return Outcome::Failed(e);
            }
            removed = true;
        }
        Ok(None) => {}
        Err(e) => return Outcome::Failed(e),
    }
    if removed {
        Outcome::Removed
    } else {
        Outcome::NothingToRemove
    }
}

/// Inspect one target without changing anything.
pub fn status(fs: &dyn HookFs) -> Vec<HookStatus> {
    HookAgent::ALL
        .iter()
        .map(|&agent| {
            let dir = match fs.dir_exists(agent.dir()) {
                Ok(dir) => dir,
                Err(e) => {
                    return HookStatus {
                        agent,
                        installed: None,
                        hooks_disabled: false,
                        error: Some(e),
                    }
                }
            };
            if !dir {
                return HookStatus {
                    agent,
                    installed: None,
                    hooks_disabled: false,
                    error: None,
                };
            }
            let version = fs
                .read(&agent.script())
                .ok()
                .flatten()
                .and_then(|s| script_version(&s));
            let entries_ok = fs
                .read(agent.settings())
                .ok()
                .flatten()
                .map(|s| merged_hooks(agent, &s) == Ok(None))
                .unwrap_or(false);
            let hooks_disabled = agent == HookAgent::Codex
                && fs
                    .read(".codex/config.toml")
                    .ok()
                    .flatten()
                    .is_some_and(|c| codex_hooks_disabled(&c));
            HookStatus {
                agent,
                installed: Some((version, entries_ok)),
                hooks_disabled,
                error: None,
            }
        })
        .collect()
}

#[cfg(test)]
#[path = "../../tests/unit/infra/agent_hooks.rs"]
mod tests;
