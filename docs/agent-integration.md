# Agent detection integration

Deck detects interactive coding agents inside tmux panes. The IO-free logic
lives in the `agent-detect` workspace crate; deck owns process and pane
collection, timeouts, remote transport, and rendering.

## Detection contract

`detect_agents(panes, ps_output)` receives tmux pane identities and a
`ps -axo pid=,ppid=,args=` snapshot. It walks each pane's process subtree
breadth-first and returns the shallowest matching agent, at most one per pane.
Processes outside a supplied pane subtree are ignored, which excludes detached
headless and IDE-extension processes.

Current interactive signatures are:

- Claude Code: executable basename `claude`, excluding `-p`, `--print`,
  `stream-json`, and extension `native-binary/claude` invocations.
- Codex: executable basename `codex`, excluding known non-interactive commands
  such as `exec`, `review`, `mcp`, `app-server`, `cloud`, `login`, and related
  service or maintenance commands. Bare Codex, prompt, `resume`, and `fork`
  forms remain interactive.

The exact exclusion list and executable matching rules are defined and tested
next to `classify` in `crates/agent-detect/src/lib.rs`. Update those tests with
every signature change so a new CLI form cannot silently add false positives.

## Status classification

`classify_status` maps a captured pane buffer to `Working`, `Idle`, `Waiting`,
or `Unknown`. Claude Code has tested spinner, interrupt-hint, completed-turn,
and confirmation-dialog rules.

Codex is read differently, and the difference matters. Claude Code's rule is
that the lowest status-bearing line wins, because anything above it is stale
transcript. Codex keeps its composer placeholder ("Ask Codex to do anything")
on screen for the whole turn and draws it *below* the "Working (1s - esc to
interrupt)" status line, so the same bottom-up rule reports every busy pane as
idle. Its classifier takes precedence by state instead -- waiting, then
working, then idle as the fallback -- over the bottom non-blank lines.

Codex fixtures are `capture-pane` output from a real session (codex-cli
0.154.0), including the approval dialog, the directory-trust prompt, and a
turn in flight. Re-capture them when the TUI changes.

Runtime wiring is split deliberately:

- `src/infra/agent.rs` collects the local process snapshot through the bounded
  command runner and re-exports the pure crate API.
- `src/system/tmux.rs` supplies local or remote pane/process data.
- `src/infra/tmux/remote.rs` batches each remote process/pane probe and agent
  pane capture over SSH before applying the shared status classifier.
- `src/infra/refresh.rs` applies excludes, captures local pane buffers, and
  classifies status before the sidebar state is updated.

When adding an agent or status rule, keep process matching in `agent-detect`,
keep shell/ssh/tmux calls in deck, and add positive and negative fixtures for
interactive and headless forms.
