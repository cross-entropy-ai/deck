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
        classify("/Users/me/.bun/.../@openai/codex-darwin-arm64/vendor/aarch64-apple-darwin/codex/codex"),
        Some(AgentKind::Codex)
    );
    // Non-interactive subcommands are not counted.
    for sub in ["exec", "review", "mcp", "mcp-server", "app-server", "remote-control", "cloud", "login"] {
        assert_eq!(classify(&format!("codex {sub} --flag")), None, "codex {sub}");
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

#[test]
fn count_agents_one_per_pane_excludes_subagents_and_headless() {
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
    let counts = count_agents(&[100, 300, 500], ps);
    assert_eq!(counts, AgentCounts { claude: 1, codex: 1 });
}

#[test]
fn count_agents_detects_agent_run_as_pane_root() {
    // Some setups exec the agent as the pane's command (pane_pid IS the
    // agent), with no intervening shell.
    let ps = "800 1 claude\n900 1 -zsh";
    let counts = count_agents(&[800, 900], ps);
    assert_eq!(counts, AgentCounts { claude: 1, codex: 0 });
}

#[test]
fn count_agents_empty_inputs() {
    assert_eq!(count_agents(&[], ""), AgentCounts::default());
    assert_eq!(count_agents(&[1, 2], ""), AgentCounts::default());
}
