//! Claude Code hook installer.
//!
//! `deck hooks install` drops our shim script into `~/.claude/hooks/`
//! and registers it in `~/.claude/settings.json` for every Claude Code
//! hook event we care about. The shim writes per-session state files
//! under `~/.config/deck/state/` which the refresh worker polls to
//! derive `SessionStatus::{Working, Waiting, Idle}` for Claude sessions.
//!
//! `deck hooks uninstall` removes only the entries we added (identified
//! by the exact shim path) and leaves everything else untouched.
//!
//! Both commands are idempotent: installing twice is a no-op, and
//! uninstalling when not installed just prints a message.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

/// The embedded shim script. Shipped as `scripts/deck-state-hook.sh`
/// and installed to `~/.claude/hooks/deck-state.sh`.
pub const SHIM_SCRIPT: &str = include_str!("../../scripts/deck-state-hook.sh");

/// The Claude hook events we register for. Kept narrow on purpose:
/// only stock events present in every Claude Code release, mapped to
/// deck's three-state model. Stays in sync with `SHIM_SCRIPT`.
const HOOK_EVENTS: &[&str] = &[
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "SubagentStop",
    "PreCompact",
    "Stop",
    "SessionStart",
    "SessionEnd",
    "Notification",
];

fn home_dir() -> io::Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME not set"))
}

fn claude_dir() -> io::Result<PathBuf> {
    Ok(home_dir()?.join(".claude"))
}

fn settings_path() -> io::Result<PathBuf> {
    Ok(claude_dir()?.join("settings.json"))
}

fn shim_install_path() -> io::Result<PathBuf> {
    Ok(claude_dir()?.join("hooks").join("deck-state.sh"))
}

/// Load and parse `~/.claude/settings.json`. Missing file → empty
/// object. Parse error propagates so we don't clobber a file the user
/// is still editing.
fn load_settings(path: &Path) -> io::Result<Value> {
    match fs::read_to_string(path) {
        Ok(content) if content.trim().is_empty() => Ok(json!({})),
        Ok(content) => serde_json::from_str(&content).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("parse {}: {}", path.display(), e),
            )
        }),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(json!({})),
        Err(e) => Err(e),
    }
}

/// Atomically write `settings.json`. Creates a `.bak` alongside the
/// original if we're about to overwrite a non-empty file, so users can
/// recover if the merge misbehaves.
fn save_settings(path: &Path, value: &Value) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    if path.exists() {
        let bak = path.with_extension("json.bak");
        let _ = fs::copy(path, bak);
    }
    let mut serialized = serde_json::to_string_pretty(value).map_err(|e| {
        io::Error::new(io::ErrorKind::InvalidData, format!("serialize: {}", e))
    })?;
    serialized.push('\n');

    let tmp = path.with_extension("json.tmp");
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(serialized.as_bytes())?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)
}

/// Write the shim script to `~/.claude/hooks/deck-state.sh` (0o755).
/// Returns the path it was written to.
fn install_shim() -> io::Result<PathBuf> {
    let path = shim_install_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, SHIM_SCRIPT)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms)?;
    }

    Ok(path)
}

fn remove_shim() -> io::Result<bool> {
    let path = shim_install_path()?;
    match fs::remove_file(&path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

/// Merge our hook registration into `settings["hooks"]`. We create a
/// dedicated matcher group per event so uninstall can remove exactly
/// our entries without touching other tools' hooks.
fn merge_install(settings: &mut Value, shim_path: &str) {
    let root = settings.as_object_mut().expect("settings is an object");
    let hooks = root
        .entry("hooks".to_string())
        .or_insert_with(|| json!({}));
    let hooks = hooks.as_object_mut().expect("hooks is an object");

    for event in HOOK_EVENTS {
        let entry = json!({
            "matcher": "*",
            "hooks": [
                {"type": "command", "command": shim_path}
            ]
        });

        let list = hooks
            .entry((*event).to_string())
            .or_insert_with(|| json!([]));

        // If the user's settings.json has this key set to something
        // other than an array (unlikely but possible with old schemas),
        // replace it — Claude Code expects an array here.
        if !list.is_array() {
            *list = json!([]);
        }

        let arr = list.as_array_mut().unwrap();
        let already_present = arr.iter().any(|group| group_uses_our_shim(group, shim_path));
        if !already_present {
            arr.push(entry);
        }
    }
}

/// Remove every hook group whose command array contains our shim path.
/// Leaves other groups (including user-authored hooks with different
/// commands) untouched.
fn merge_uninstall(settings: &mut Value, shim_path: &str) -> usize {
    let Some(root) = settings.as_object_mut() else {
        return 0;
    };
    let Some(hooks) = root.get_mut("hooks").and_then(|v| v.as_object_mut()) else {
        return 0;
    };

    let mut removed = 0usize;
    for event in HOOK_EVENTS {
        if let Some(list) = hooks.get_mut(*event).and_then(|v| v.as_array_mut()) {
            let before = list.len();
            list.retain(|group| !group_uses_our_shim(group, shim_path));
            removed += before - list.len();
        }
    }

    // Prune empty event arrays and the "hooks" key itself if it ended
    // up empty — keeps user settings.json tidy.
    let keys: Vec<String> = hooks
        .iter()
        .filter_map(|(k, v)| v.as_array().filter(|a| a.is_empty()).map(|_| k.clone()))
        .collect();
    for k in keys {
        hooks.remove(&k);
    }
    if hooks.is_empty() {
        root.remove("hooks");
    }

    removed
}

fn group_uses_our_shim(group: &Value, shim_path: &str) -> bool {
    group
        .get("hooks")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter().any(|cmd| {
                cmd.get("command")
                    .and_then(|c| c.as_str())
                    .is_some_and(|c| c == shim_path)
            })
        })
        .unwrap_or(false)
}

/// CLI entry point for `deck hooks install`.
pub fn run_install() -> io::Result<()> {
    // jq is required by the shim; warn up front if it's missing so the
    // user doesn't wonder why no state files appear later.
    if !jq_available() {
        eprintln!(
            "deck: warning: `jq` is not in PATH. The hook shim needs it \
             at runtime.\n  macOS:   brew install jq\n  Debian:  apt-get install jq"
        );
    }

    let shim = install_shim()?;
    let shim_str = shim
        .to_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "non-utf8 path"))?
        .to_string();

    let settings = settings_path()?;
    let mut value = load_settings(&settings)?;
    if !value.is_object() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "~/.claude/settings.json is not a JSON object",
        ));
    }
    merge_install(&mut value, &shim_str);
    save_settings(&settings, &value)?;

    println!("deck: installed hook script at {}", shim.display());
    println!("deck: updated {}", settings.display());
    println!("deck: restart any running Claude Code sessions to pick up the hook.");
    Ok(())
}

/// CLI entry point for `deck hooks uninstall`.
pub fn run_uninstall() -> io::Result<()> {
    let shim = shim_install_path()?;
    let shim_str = shim
        .to_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "non-utf8 path"))?
        .to_string();

    let settings = settings_path()?;
    let mut value = load_settings(&settings)?;
    let removed = if value.is_object() {
        merge_uninstall(&mut value, &shim_str)
    } else {
        0
    };
    if removed > 0 {
        save_settings(&settings, &value)?;
    }

    let shim_removed = remove_shim()?;

    match (removed, shim_removed) {
        (0, false) => println!("deck: no hooks installed — nothing to do."),
        _ => {
            if removed > 0 {
                println!("deck: removed {removed} hook entry/entries from {}", settings.display());
            }
            if shim_removed {
                println!("deck: removed shim script at {}", shim.display());
            }
        }
    }
    Ok(())
}

fn jq_available() -> bool {
    std::process::Command::new("jq")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(test)]
#[path = "../../tests/unit/infra/hooks.rs"]
mod tests;
