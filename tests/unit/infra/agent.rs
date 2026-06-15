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
        pane(100, "deck", "1", "0"),
        pane(300, "work", "2", "1"),
        pane(500, "work", "2", "2"),
    ];
    let agents = detect_agents(&panes, ps);
    assert_eq!(agents.len(), 2);
    // pane 100 -> claude, located at its session/window/pane, with the
    // stable pane id carried for switching.
    assert_eq!(agents[0].kind, AgentKind::Claude);
    assert_eq!(agents[0].location(), "deck:1.0");
    assert_eq!(agents[0].pane_id, "%100");
    // pane 300 -> codex (matched two levels down the wrapper).
    assert_eq!(agents[1].kind, AgentKind::Codex);
    assert_eq!(agents[1].location(), "work:2.1");
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
fn excluded_session_agents_are_filtered_out() {
    // `collect_local` runs this exact retain after detection so an agent
    // in a hidden session never reaches the sidebar footer.
    use crate::config;
    let panes = [pane(100, "work", "1", "0"), pane(200, "_hidden", "1", "0")];
    let ps = "100 1 -zsh\n150 100 claude\n200 1 -zsh\n250 200 claude";
    let mut agents = detect_agents(&panes, ps);
    assert_eq!(agents.len(), 2, "both detected before filtering");

    let compiled = config::compile_patterns(&["_*".to_string()]);
    agents.retain(|a| !config::session_excluded(&a.session, &compiled));

    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0].session, "work", "the '_hidden' agent is excluded");
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
    assert_eq!(classify_status(AgentKind::Claude, tool), AgentStatus::Working);

    // A "… (ctrl+o to expand)" tool-result tail is NOT a live timer → not
    // working on its own (no interrupt hint, paren isn't a duration).
    let collapsed = "⏺ Read(config.yaml)\n  ⎿ Read 1 file… (ctrl+o to expand)\n│ > │\n? for shortcuts";
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
