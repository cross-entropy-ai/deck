use super::*;
use serde_json::json;

#[test]
fn install_merges_into_empty_settings() {
    let mut settings = json!({});
    merge_install(&mut settings, "/home/me/.claude/hooks/deck-state.sh");

    for event in HOOK_EVENTS {
        let list = settings["hooks"][event]
            .as_array()
            .expect("event list present");
        assert_eq!(list.len(), 1, "event {event} should have one group");
        assert_eq!(list[0]["matcher"], "*");
        assert_eq!(
            list[0]["hooks"][0]["command"],
            "/home/me/.claude/hooks/deck-state.sh"
        );
    }
}

#[test]
fn install_is_idempotent() {
    let shim = "/home/me/.claude/hooks/deck-state.sh";
    let mut settings = json!({});
    merge_install(&mut settings, shim);
    merge_install(&mut settings, shim);

    for event in HOOK_EVENTS {
        let list = settings["hooks"][event].as_array().unwrap();
        let ours = list
            .iter()
            .filter(|g| group_uses_our_shim(g, shim))
            .count();
        assert_eq!(ours, 1, "event {event}: duplicate after re-install");
    }
}

#[test]
fn install_preserves_third_party_entries() {
    let shim = "/home/me/.claude/hooks/deck-state.sh";
    let mut settings = json!({
        "hooks": {
            "PreToolUse": [
                {
                    "matcher": "Bash",
                    "hooks": [{"type": "command", "command": "/opt/audit.sh"}]
                }
            ]
        },
        "other_key": "preserved"
    });
    merge_install(&mut settings, shim);

    let list = settings["hooks"]["PreToolUse"].as_array().unwrap();
    assert_eq!(list.len(), 2, "audit.sh should still be there alongside ours");
    assert!(list
        .iter()
        .any(|g| g["hooks"][0]["command"] == "/opt/audit.sh"));
    assert_eq!(settings["other_key"], "preserved");
}

#[test]
fn uninstall_removes_only_our_entries() {
    let shim = "/home/me/.claude/hooks/deck-state.sh";
    let mut settings = json!({
        "hooks": {
            "PreToolUse": [
                {
                    "matcher": "Bash",
                    "hooks": [{"type": "command", "command": "/opt/audit.sh"}]
                },
                {
                    "matcher": "*",
                    "hooks": [{"type": "command", "command": shim}]
                }
            ]
        }
    });

    let removed = merge_uninstall(&mut settings, shim);
    assert_eq!(removed, 1);

    let list = settings["hooks"]["PreToolUse"].as_array().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["hooks"][0]["command"], "/opt/audit.sh");
}

#[test]
fn uninstall_prunes_empty_hooks_key() {
    let shim = "/home/me/.claude/hooks/deck-state.sh";
    let mut settings = json!({});
    merge_install(&mut settings, shim);
    merge_uninstall(&mut settings, shim);
    assert!(
        settings.get("hooks").is_none(),
        "empty hooks object should be pruned"
    );
}

#[test]
fn uninstall_on_clean_settings_is_noop() {
    let mut settings = json!({"theme": "dark"});
    let removed = merge_uninstall(&mut settings, "/anything");
    assert_eq!(removed, 0);
    assert_eq!(settings["theme"], "dark");
}
