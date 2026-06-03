# Agent Integration — TODO / Plan

> Status: **planning only — do not implement yet.** Requirements captured
> from the user; implementation will be done step by step under the
> user's direction.

## Goal

Make deck aware of coding agents running inside its tmux sessions, so the
sidebar shows what each agent is doing, alerts when one needs attention,
and lets the user act on it without leaving deck.

## Targets

- **Claude Code** (CLI)
- **Codex CLI**

(Design the detection layer so adding another agent later is mechanical,
but only these two are in scope now.)

## Capabilities (the TODO list)

### 1. Show agent activity & status
- [ ] Detect, per session, that a coding agent is running and which one
      (Claude Code vs Codex).
- [ ] Track richer state than today's `Working`/`Idle`. At minimum:
      working, idle/done, **needs user input**, **waiting for tool
      approval**.
- [ ] Render that state in the sidebar row (icon/label/color), distinct
      from a plain shell session.

### 2. Notifications
- [ ] Raise a notification when an agent needs attention (needs input /
      approval pending / finished).
- [ ] Route the user to the agent that needs attention.

### 3. Click an agent → switch to it
- [ ] Clicking an agent row switches to that session (deck already
      switches sessions on click — confirm this covers it, plus any
      "jump to the agent needing attention" shortcut).

### 4. Approve / deny from deck
- [ ] Approve or deny an agent's pending tool-use / permission prompt
      from the sidebar, without switching into the pane.

## Research findings — detecting agents & locating them (no hooks)

Goal of this step: with **no hook installation**, find how many Claude
Code / Codex instances are running **interactively in tmux** (local and
on remote hosts), and get the unique tmux `(session, window, pane)` IDs
so deck can switch to the session+window and focus the right pane.

### Detection model (validated on this machine)

1. Enumerate every pane on the tmux server deck is attached to:
   `tmux list-panes -a -F '#{pane_pid}\t#{session_id}\t#{window_id}\t#{pane_id}\t#{session_name}\t#{window_index}\t#{pane_current_path}'`
2. Take one process snapshot: `ps -axo pid=,ppid=,args=`.
3. For each pane, DFS the process subtree rooted at `pane_pid` and match
   an agent by its `argv` (see signatures). The matched pane's
   `(session_id, window_id, pane_id)` is the agent's location.
4. Remote hosts: identical, prefixed with `ssh <host>` — deck already
   has this plumbing in `remote_tmux`. No hooks, purely observational.

Verified live: 3 interactive Claude Code instances were found and located
to their exact panes; the headless IDE-extension claudes/codexes were
correctly excluded (they aren't under any tmux `pane_pid`).

### Why not `pane_current_command`

- For a Claude Code pane it shows the **version string** (e.g.
  `2.1.160`) — it changes every release, and flips to `node`/`zsh`/a tool
  name while the agent runs a subprocess.
- It **misses** an agent that's momentarily foregrounding a child: a pane
  whose `pane_current_command` was `zsh` actually had a `claude` running
  underneath. The subtree walk catches it; the current-command heuristic
  doesn't.

### tmux unique IDs (stable for the server's lifetime)

`#{session_id}` → `$N` (e.g. `$83`), `#{window_id}` → `@N` (e.g. `@136`),
`#{pane_id}` → `%N` (e.g. `%255`). `#{pane_pid}` links a pane to the `ps`
tree. These IDs are the switch/focus targets (below) and survive renames.

### Claude Code signature

- Native binary: `~/.local/bin/claude` (Mach-O). The agent process is a
  **child of the pane's shell**, `argv` = `claude …` (e.g.
  `claude --dangerously-skip-permissions`). `pane_pid` itself is the
  `-zsh` login shell, not the agent.
- Match: `argv[0]` basename == `claude`.
- **Interactive** = NOT headless. Exclude `-p` / `--print` /
  `--output-format stream-json`, and the IDE-extension form
  `…/native-binary/claude --output-format stream-json …`.
- Note: kernel `comm` (what tmux reads) = the version string; `ps comm` =
  `claude`; only `ps args` is reliable.

### Codex CLI signature

- PATH `codex` (`~/.bun/bin/codex`) is a **Node wrapper** (`@openai/codex`)
  that `spawn`s the platform-native binary
  (`@openai/codex-<target>/vendor/.../codex`). So the tree has a `node`
  wrapper → native `codex`. Match the **native `codex`** (argv[0]
  basename `codex`).
- Interactive = bare `codex` / `codex [PROMPT]`, or `codex resume` /
  `codex fork`.
- Exclude non-interactive / headless subcommands: `exec`, `review`,
  `mcp`, `mcp-server`, `app-server`, `remote-control`, `cloud`, `app`,
  `login`, `logout`, `completion`, `update`, `doctor`, `sandbox`,
  `debug`, `apply`, `plugin`. (The running headless one observed was
  `…/codex app-server`.)
- Not yet verified against a *live* interactive codex (none was running);
  confirm the native process's `argv` shape when we implement.

### Switch + focus (using the IDs)

- Local: `tmux switch-client -c <client-tty> -t <session_id>` (deck
  already switches by tty), then `tmux select-window -t <pane_id>` +
  `tmux select-pane -t <pane_id>` to land on the exact pane. A
  `pane_id`/`window_id` resolves its window/session, so no name ambiguity.
- Remote: same selects over `ssh <host> tmux …` plus deck's existing
  remote-attach for that host.

### Caveats

- `list-panes -a` covers the **one tmux server** deck is attached to
  (default socket). Agents under a different `tmux -L`/`-S` socket aren't
  seen — out of scope.
- Detection is a snapshot; refresh on deck's existing cadence.

## Open questions (to resolve with the user during implementation)

- **Fine-grained state** (working / idle-done / needs-input / awaiting
  approval) is the *next* research step — without hooks it likely comes
  from `capture-pane` output parsing (prompt/approval-menu patterns)
  and/or the foreground-subprocess heuristic. Not investigated yet.
- **What counts as "needs attention"**, and which notification channels:
  in-app badge, terminal bell, macOS notification.
- **Approve/deny wiring** — likely the hardest/most fragile: send
  keystrokes to the pane (write to PTY) vs. any agent-provided IPC.
  Per-agent key sequences; screen-state dependence.
- **Local vs remote** — does agent state need to work for remote
  sessions too, or local-only first?
- Scope of "switch on click" beyond what deck already does.
