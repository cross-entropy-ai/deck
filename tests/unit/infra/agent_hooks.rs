use super::*;

#[test]
fn merged_hooks_from_empty_creates_all_events() {
    let merged = merged_hooks(HookAgent::Claude, "").unwrap().unwrap();
    let v: serde_json::Value = serde_json::from_str(&merged).unwrap();
    let hooks = v.get("hooks").unwrap().as_object().unwrap();
    for event in [
        "SessionStart",
        "UserPromptSubmit",
        "PermissionRequest",
        "Stop",
        "SessionEnd",
    ] {
        let entries = hooks.get(event).unwrap().as_array().unwrap();
        assert_eq!(entries.len(), 1, "{event}");
        assert!(is_deck_entry(&entries[0]), "{event}");
    }
    // The command leaves $HOME unexpanded so it is machine-independent.
    assert!(merged.contains("sh \\\"$HOME/.claude/deck-agent-state.sh\\\" working"));
    // Trailing newline: the file ends like a text file.
    assert!(merged.ends_with('\n'));
}

#[test]
fn merged_hooks_is_byte_stable_once_installed() {
    // The idempotence the Codex trust gate demands: a second merge over an
    // already-correct file must be a no-op, even though our entries aren't
    // at the positions a naive remove-and-append would put them.
    let first = merged_hooks(HookAgent::Codex, "").unwrap().unwrap();
    assert_eq!(merged_hooks(HookAgent::Codex, &first), Ok(None));

    // A user entry appended AFTER ours must not shuffle ours back to the end.
    let mut v: serde_json::Value = serde_json::from_str(&first).unwrap();
    v["hooks"]["Stop"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({"hooks": [{"type": "command", "command": "mine"}]}));
    let with_user = serde_json::to_string_pretty(&v).unwrap();
    assert_eq!(merged_hooks(HookAgent::Codex, &with_user), Ok(None));
}

#[test]
fn merged_hooks_preserves_user_entries_and_replaces_stale_deck_ones() {
    let existing = r#"{
        "model": "opus",
        "hooks": {
            "Stop": [
                {"hooks": [{"type": "command", "command": "user-thing"}]},
                {"_deck": true, "hooks": [{"type": "command", "command": "old shape", "timeout": 9}]}
            ]
        }
    }"#;
    let merged = merged_hooks(HookAgent::Claude, existing).unwrap().unwrap();
    let v: serde_json::Value = serde_json::from_str(&merged).unwrap();
    // Unrelated top-level keys survive.
    assert_eq!(v["model"], "opus");
    let stop = v["hooks"]["Stop"].as_array().unwrap();
    // The user's entry survives; the stale deck entry is replaced by one
    // current one.
    assert_eq!(stop.len(), 2);
    assert_eq!(stop[0]["hooks"][0]["command"], "user-thing");
    assert!(is_deck_entry(&stop[1]));
    assert!(stop[1]["hooks"][0]["command"]
        .as_str()
        .unwrap()
        .ends_with("deck-agent-state.sh\" idle"));
}

#[test]
fn merged_hooks_rejects_garbage_instead_of_clobbering() {
    assert!(merged_hooks(HookAgent::Claude, "not json").is_err());
    assert!(merged_hooks(HookAgent::Claude, "[]").is_err());
}

#[test]
fn stripped_hooks_removes_only_deck_entries() {
    let installed = merged_hooks(
        HookAgent::Claude,
        r#"{"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "user-thing"}]}]}}"#,
    )
    .unwrap()
    .unwrap();
    let stripped = stripped_hooks(&installed).unwrap().unwrap();
    let v: serde_json::Value = serde_json::from_str(&stripped).unwrap();
    let hooks = v["hooks"].as_object().unwrap();
    // Events that held only deck entries lose the key entirely …
    assert!(!hooks.contains_key("SessionStart"));
    assert!(!hooks.contains_key("SessionEnd"));
    // … the user's entry stays.
    let stop = hooks["Stop"].as_array().unwrap();
    assert_eq!(stop.len(), 1);
    assert_eq!(stop[0]["hooks"][0]["command"], "user-thing");
    // Stripping a deck-free file is a no-op.
    assert_eq!(stripped_hooks(&stripped), Ok(None));
    assert_eq!(stripped_hooks(""), Ok(None));
}

#[test]
fn codex_hooks_disabled_scan() {
    assert!(codex_hooks_disabled("[features]\nhooks = false\n"));
    assert!(codex_hooks_disabled(
        "model = \"x\"\n[features]\nmemories = true\nhooks = false # off\n"
    ));
    assert!(!codex_hooks_disabled("[features]\nhooks = true\n"));
    assert!(!codex_hooks_disabled(""));
    // Only the top-level [features] table counts.
    assert!(!codex_hooks_disabled("[other]\nhooks = false\n"));
    // The key elsewhere is not the feature switch.
    assert!(!codex_hooks_disabled("hooks = false\n"));
}

#[test]
fn script_carries_the_declared_version() {
    // The rust-side constant and the script header must move together —
    // a mismatch means someone bumped one and not the other.
    assert_eq!(
        script_version(HOOK_SCRIPT).as_deref(),
        Some(DECK_HOOK_VERSION)
    );
}

/// An in-memory HookFs to drive install/uninstall end to end.
#[derive(Default)]
struct FakeFs {
    dirs: std::cell::RefCell<Vec<String>>,
    files: std::cell::RefCell<std::collections::HashMap<String, String>>,
    writes: std::cell::RefCell<usize>,
}

impl HookFs for FakeFs {
    fn dir_exists(&self, rel: &str) -> Result<bool, String> {
        Ok(self.dirs.borrow().iter().any(|d| d == rel))
    }
    fn read(&self, rel: &str) -> Result<Option<String>, String> {
        Ok(self.files.borrow().get(rel).cloned())
    }
    fn write(&self, rel: &str, content: &str, _executable: bool) -> Result<(), String> {
        *self.writes.borrow_mut() += 1;
        self.files.borrow_mut().insert(rel.into(), content.into());
        Ok(())
    }
    fn remove(&self, rel: &str) -> Result<(), String> {
        self.files.borrow_mut().remove(rel);
        Ok(())
    }
}

#[test]
fn install_is_idempotent_and_skips_absent_agents() {
    let fs = FakeFs::default();
    fs.dirs.borrow_mut().push(".claude".into());
    // No .codex dir → codex skipped entirely.
    let reports = install(&fs);
    assert_eq!(reports[0].outcome, Outcome::Installed);
    assert_eq!(reports[1].outcome, Outcome::Absent);
    let first_writes = *fs.writes.borrow();
    assert_eq!(first_writes, 2, "script + settings");

    // Second install: byte-stable, zero writes (the Codex trust gate rule).
    let reports = install(&fs);
    assert_eq!(reports[0].outcome, Outcome::Unchanged);
    assert_eq!(*fs.writes.borrow(), first_writes);

    // Uninstall removes ours and only ours, then has nothing left to do.
    let reports = uninstall(&fs);
    assert_eq!(reports[0].outcome, Outcome::Removed);
    let reports = uninstall(&fs);
    assert_eq!(reports[0].outcome, Outcome::NothingToRemove);
}

#[test]
fn install_converges_even_when_reads_trim_trailing_newlines() {
    // The ssh read path (`stdout_trimmed`) strips the script's trailing
    // newline; a reinstall must still see it as current.
    struct TrimmingFs(FakeFs);
    impl HookFs for TrimmingFs {
        fn dir_exists(&self, rel: &str) -> Result<bool, String> {
            self.0.dir_exists(rel)
        }
        fn read(&self, rel: &str) -> Result<Option<String>, String> {
            Ok(self.0.read(rel)?.map(|s| s.trim_end().to_string()))
        }
        fn write(&self, rel: &str, content: &str, executable: bool) -> Result<(), String> {
            self.0.write(rel, content, executable)
        }
        fn remove(&self, rel: &str) -> Result<(), String> {
            self.0.remove(rel)
        }
    }
    let fs = TrimmingFs(FakeFs::default());
    fs.0.dirs.borrow_mut().push(".codex".into());
    install(&fs);
    let writes = *fs.0.writes.borrow();
    let reports = install(&fs);
    assert_eq!(reports[1].outcome, Outcome::Unchanged);
    assert_eq!(*fs.0.writes.borrow(), writes, "reinstall wrote nothing");
}

#[test]
fn status_reads_version_entries_and_codex_feature_switch() {
    let fs = FakeFs::default();
    fs.dirs.borrow_mut().push(".codex".into());
    fs.files.borrow_mut().insert(
        ".codex/config.toml".into(),
        "[features]\nhooks = false\n".into(),
    );
    install(&fs);
    let st = status(&fs);
    assert!(st[0].installed.is_none(), "no ~/.claude");
    let codex = &st[1];
    let (version, entries_ok) = codex.installed.as_ref().unwrap();
    assert_eq!(version.as_deref(), Some(DECK_HOOK_VERSION));
    assert!(entries_ok);
    assert!(codex.hooks_disabled, "config.toml turns hooks off");
}
