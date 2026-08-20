use super::*;

fn cap(host: Option<&str>, id: &str, content: &str) -> PaneCapture {
    PaneCapture {
        host: host.map(str::to_string),
        id: id.to_string(),
        content: content.to_string(),
    }
}

#[test]
fn build_prompt_splices_session_blocks_at_placeholder() {
    let template = "intro\n{{SESSIONS}}\noutro";
    let out = build_prompt(
        template,
        &[
            cap(None, "deck:1.1", "local buffer"),
            cap(Some("h1"), "work:0.2", "remote buffer"),
        ],
    );
    assert!(out.starts_with("intro\n"));
    assert!(out.ends_with("\noutro"));
    assert!(out.contains("<session id=\"deck:1.1\">\nlocal buffer\n</session>"));
    assert!(out.contains("<session id=\"work:0.2\" host=\"h1\">\nremote buffer\n</session>"));
    // The placeholder itself is gone.
    assert!(!out.contains(PROMPT_PLACEHOLDER));
}

#[test]
fn build_prompt_appends_when_no_placeholder() {
    let out = build_prompt("just a prompt", &[cap(None, "a:0.0", "buf")]);
    assert!(out.starts_with("just a prompt\n\n"));
    assert!(out.contains("<session id=\"a:0.0\">\nbuf\n</session>"));
}

#[test]
fn build_prompt_escapes_attribute_specials() {
    let out = build_prompt(
        "{{SESSIONS}}",
        &[cap(Some("a&b<\"c"), "x\"y", "content & <stuff>")],
    );
    // Attributes are escaped...
    assert!(out.contains("host=\"a&amp;b&lt;&quot;c\""));
    assert!(out.contains("id=\"x&quot;y\""));
    // ...but the content is left raw for the model.
    assert!(out.contains("content & <stuff>"));
}

#[test]
fn language_label_shows_default_for_empty() {
    assert_eq!(language_label(""), "Default");
    assert_eq!(language_label("中文"), "中文");
}

#[test]
fn nested_deck_detection_covers_every_icon_compatibility_style() {
    for header in ["▤ Sessions 4", "# Sessions 4", "\u{e795} Sessions 4"] {
        assert!(is_nested_deck(header), "missing marker for {header:?}");
    }
    assert!(!is_nested_deck("ordinary agent output"));
}

#[test]
fn summary_commands_match_each_agent_cli() {
    use crate::summary_card::SummaryAgent;

    let (claude, claude_name) = summary_command(SummaryAgent::Claude, "haiku");
    assert_eq!(claude_name, "claude");
    assert_eq!(claude.get_program(), "claude");
    assert_eq!(
        claude.get_args().collect::<Vec<_>>(),
        ["-p", "--model", "haiku"]
    );

    let (codex, codex_name) = summary_command(SummaryAgent::Codex, "haiku");
    assert_eq!(codex_name, "codex");
    assert_eq!(codex.get_program(), "codex");
    let args = codex.get_args().collect::<Vec<_>>();
    assert!(args.contains(&std::ffi::OsStr::new("--ephemeral")));
    assert!(args.contains(&std::ffi::OsStr::new("read-only")));
    assert_eq!(args.last().copied(), Some(std::ffi::OsStr::new("-")));
    assert!(!args.contains(&std::ffi::OsStr::new("haiku")));
}

#[test]
fn log_dir_is_under_home_cache_not_tmp() {
    // The dump embeds captured pane buffers; it must not live in a shared,
    // world-readable /tmp dir.
    let dir = log_dir();
    assert!(
        dir.ends_with(".cache/deck/summary"),
        "expected ~/.cache/deck/summary, got {dir:?}"
    );
    assert!(!dir.starts_with("/tmp"), "must not be under /tmp: {dir:?}");
}

#[test]
fn write_log_entry_disabled_writes_nothing() {
    let dir = std::env::temp_dir().join(format!("deck-summary-test-off-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    write_log_entry(&dir, false, 1, "secret pane contents");
    assert!(
        !dir.exists(),
        "logging disabled must not create the dir or write a file"
    );
}

#[cfg(unix)]
#[test]
fn write_log_entry_enabled_is_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("deck-summary-test-perm-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    write_log_entry(&dir, true, 42, "body");
    let file = dir.join("summary-42.md");
    assert!(file.exists(), "enabled logging should write the entry");
    let fmode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o777;
    assert_eq!(fmode, 0o600, "log file must be owner read/write only");
    let dmode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
    assert_eq!(dmode, 0o700, "log dir must be owner-only");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn prune_logs_keeps_newest_n() {
    let dir = std::env::temp_dir().join(format!("deck-summary-test-prune-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for i in 1000..1025 {
        std::fs::write(dir.join(format!("summary-{i}.md")), "x").unwrap();
    }
    prune_logs(&dir, 20);
    let names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().into_string().unwrap())
        .collect();
    assert_eq!(names.len(), 20, "should retain exactly 20");
    assert!(
        names.contains(&"summary-1024.md".to_string()),
        "keeps newest"
    );
    assert!(
        !names.contains(&"summary-1000.md".to_string()),
        "drops oldest"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn run_command_cancel_kills_the_child_and_returns_cancelled() {
    use std::process::Command;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;
    use std::time::Instant;

    // A stub that ignores stdin and sleeps far longer than the test would
    // tolerate, standing in for a hung `claude`.
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg("sleep 30");

    // Pre-cancelled: the first poll iteration must kill the child and bail
    // — not wait out the sleep or the 90s SUMMARY_TIMEOUT.
    let cancel = Cancel::from_flag(Arc::new(AtomicBool::new(true)));

    let start = Instant::now();
    let result = run_command(cmd, "stub", "prompt", &cancel);
    assert_eq!(
        result,
        Err(CANCELLED_MSG.to_string()),
        "cancel returns the sentinel"
    );
    assert!(
        start.elapsed().as_secs() < 5,
        "cancel must be prompt, took {:?}",
        start.elapsed()
    );
}
