# Agent status: three-source detection plan

Plan of record for upgrading agent status detection from screen-scraping alone
to three merged sources: the pane buffer (screen), the tmux activity clock, and
optional agent lifecycle hooks. Companion to `agent-integration.md`, which
documents the current detection contract; this file records what we measured,
what we decided, and the implementation sequence.

## Background: what we measured (2026-08-24)

Experiments against Claude Code 2.1.241 and Codex CLI 0.149.1, each driven in
an isolated tmux server via `send-keys`/`capture-pane`, with logger hooks
subscribed to every lifecycle event. Findings that shape the design:

### Hooks are edge-triggered and miss every user-driven termination

| Scenario | Claude Code | Codex |
|---|---|---|
| Normal turn | `UserPromptSubmit` → `Stop` | same |
| **Esc interrupt mid-turn** | **no event at all** | **no event at all** |
| Permission dialog appears | `PermissionRequest` (before the dialog is drawn, ~6 s ahead of `Notification`) | same (same second as `PreToolUse`) |
| **User denies the permission** | **no event** (checked 64 s later) | **no event** |
| Ctrl+C ×2 exit | `SessionEnd` | `SessionEnd` |
| SIGHUP (`tmux kill-server`) | `SessionEnd` | **nothing** |
| `kill -9` | nothing (but the process leaves the pane tree, so `detect_agents` drops the row anyway) | same |

`Stop` fires only when the model finishes on its own. So a hook may *light up*
Working but can never be trusted to *retract* it: the only stuck-Working
exposure is "agent alive, turn silently terminated" (Esc / deny), which is an
everyday action.

Additional per-agent quirks, all reproduced:

- Claude emits a **phantom `SubagentStop`** ~3 s after `Stop` with no subagent
  involved. Ignore `SubagentStop` and any payload carrying `agent_id`.
- Codex defers **`SessionStart` to the first turn** — 27 s idle produced no
  event; it arrived in the same second as the first `UserPromptSubmit`. A
  freshly launched idle Codex has no hook state and no identity.
- Codex clamps **`SessionEnd` hook timeouts to 3 s** (prints a warning).
- Codex has a **hook trust gate**: after hooks are installed or changed, the
  next launch blocks on a modal — "N hooks are new or changed. Hooks can run
  outside the sandbox after you trust them" with a *Continue without trusting
  (hooks won't run)* option. Unchanged content does not re-prompt. Claude has
  no equivalent gate.

### The screen is blind while a turn streams text

While Claude/Codex streams a plain text answer (no tool, no thinking), the
bottom of the pane holds only content and the input box — no spinner, no
`esc to interrupt`, no timer. `claude_classify`'s tail scan finds nothing and
falls through to `Idle`. The screen is therefore *not* a complete Working
source either. The two gaps do not overlap: hooks miss user-driven
termination, the screen misses streaming.

### `#{window_activity}` is a level-triggered output clock

tmux has no per-pane activity format (`pane_activity`, `pane_last_activity`,
`pane_written` are all empty), but `#{window_activity}` tracks live output:
while an agent streams it equals wall-clock now; once the turn ends it
freezes. `#{history_size}` is useless here because agent TUIs hold the
alternate screen. This is deck's free stand-in for the signal that lets
herdr (which owns its PTYs) skip tool-level hooks entirely: herdr's own code
says "PTY activity is the normal working authority", and its `HOOK_REMOVALS`
list shows it once installed `PreToolUse`/`PostToolUse`/`Stop` state hooks and
deliberately removed them all, keeping only `SessionStart → session` identity.
We adopt the same division of labor, with `window_activity` as the authority.

Caveat: the clock is **window**-scoped. Only trust it when
`#{window_panes} == 1`; otherwise fall back to screen-only.

### Prior art on the Codex trust gate

herdr neither mentions nor handles the gate anywhere (repo-wide search): its
Codex integration reports session identity only, so an untrusted hook costs it
nothing. Its implicit mitigation is **timing** — installs happen only on an
explicit user command (`herdr integration install codex`), so the modal
appears right after the user's own action and needs no explanation. We copy
the timing rule; we cannot copy the indifference, because our hooks carry
state.

## Architecture: merge rule

One pure function in `agent-detect` (screen classification stays IO-free and
shared by the local and ssh gathering paths):

```
merge(prev, screen: Verdict, activity_fresh: Option<bool>, hook: Option<HookReport>)
```

Priority, highest first (as implemented in `merge_status`):

1. `screen.visible_blocker` → **Waiting**. A dialog visibly on screen is the
   strongest truth and may override a non-blocked hook state.
2. `screen.keep_previous` → keep `prev` (transcript viewer, model picker —
   screens that show history instead of live state). The pure layer has no
   memory, so this rides as a flag on `DetectedAgent` and the stateful
   snapshot-apply (`app/refresh.rs`) resolves it against the old status.
3. Screen working tell, or `activity_fresh == Some(true)` → **Working**.
   This covers the streaming blind spot — and it sits *above* the idle
   tells deliberately: a previous turn's "…ed for N" line lingers on screen
   while the next turn streams, and fresh output disproves it. (An earlier
   draft put the idle tells first; that ordering re-opens the blind spot
   whenever a completed line is anywhere in the tail.)
4. Screen shows a positive **idle tell** ("…ed for N", `Interrupted ·`,
   `■ Conversation interrupted`) → **Idle**, overriding a hook's Working.
   This kills the stuck green dot after Esc / permission-deny: by then the
   activity clock has gone stale too, so rule 3 no longer holds the light.
5. Fresh hook report → hook's state (PR 4).
6. Otherwise the weak screen reading passes through (weak Waiting stays
   Waiting; unrecognized → Idle; empty capture → Unknown).

In one line: **hooks and the activity clock light Working up; only the screen
retracts it.** Lighting tolerates error (worst case the user clicks into an
idle pane); retraction must be reliable.

## Implementation sequence

Each step is a `feature/` branch + PR into `main`. Steps 1–4 touch no user
configuration and need no consent; step 5 is the only one with external side
effects and sits behind its own config flag.

### PR 1 — screen-side completion + Codex classifier

`crates/agent-detect/src/lib.rs`.

- New entry point `classify_verdict(kind, buffer, title)` returning a
  `Verdict { status, keep_previous, visible_blocker, visible_idle }`
  (modeled on herdr's `AgentDetection`); `classify_status` stays as the
  status-only wrapper so existing call sites keep working. The `title`
  parameter is the OSC pane-title tier from herdr's manifests — a
  braille/half-circle spinner → Working, Codex's "Action Required" →
  blocked. PR 1 defines and tests the tier; PR 2 wires `#{pane_title}`
  through. Two deviations from herdr, both forced by measurement or by
  reading through tmux: their `osc_title_idle` rules (any non-spinner title
  → positive idle) are dropped entirely — a tmux pane title can be a stale
  shell-set one the agent never touched, and Claude 2.1.241 shows
  "✳ <summary>" *mid-turn*, so a title must never count as idle evidence
  (see the title findings under PR 2).
- Claude, three new rules (all captured in the experiments):
  `Interrupted · What should Claude do instead?` → Idle;
  workspace-trust dialog (`1. Yes, I trust this folder` + `Enter to confirm ·
  Esc to cancel`) → Waiting + visible_blocker;
  transcript viewer / model picker → keep_previous.
- Codex, replacing the hardcoded `Unknown`:
  `esc to interrupt` → Working (the existing Claude substring already
  matches Codex's `• Working (13s • esc to interrupt)`);
  approval dialog (`Would you like to run the following command?` +
  `› 1. Yes, proceed` + `Press enter to confirm or esc to cancel`) →
  Waiting + visible_blocker;
  directory-trust dialog (herdr's `trust_directory` rule: `> You are in …` +
  `Do you trust the contents of this directory?`) → Waiting + visible_blocker;
  hook trust gate (`Hooks need review` + `Trust all and continue`) →
  Waiting + visible_blocker (herdr has no such rule; we need it because we
  will cause that screen ourselves);
  `■ Conversation interrupted - tell the model what to do differently` → Idle.
- Fixtures from the real captures: streaming mid-turn, post-interrupt,
  approval dialog, trust dialogs, completed turn — per agent.

Ships standalone: it fixes two live bugs (Codex permanently gray; trust,
approval, and interrupt screens misread). The streaming blind spot is closed
by the later PRs: the title tier once PR 2 wires it, hooks and the activity
clock for agents whose titles don't spin.

### PR 2 — activity clock + pane title

- Extend `PANE_FORMAT` (`src/infra/parser/pane.rs`) with
  `#{window_activity}`, `#{window_panes}`, and `#{pane_title}`;
  `parse_panes` tolerates both field counts (older remote tmux → `None` →
  today's behavior). The title feeds `classify_verdict`'s title tier.
- Title assumptions, measured live on Claude 2.1.241 (2026-08-24): the
  title does **not** spin — it stays "✳ <session summary>" straight through
  a running turn (tool phase and plain-text streaming alike). Two
  consequences already applied in PR 1: the spinner→working tier is
  legacy-version-only for Claude (inert on 2.1.241, kept for the versions
  herdr documented), and herdr's `osc_title_idle` ("✳ " → idle) is **wrong**
  for tmux consumers — "✳ " mid-turn would retract a correct hook Working,
  so it is dropped. Codex title behavior (braille spinner, "Action
  Required") — the spinner half verified live on codex-cli 0.149.1
  (2026-08-24): title cycles "work" → "⠧ work" mid-turn (spinning through
  the streaming phase, which the buffer misses) → "work" at completion,
  while `window_activity` tracked output and froze at the end. "Action
  Required" remains herdr's word; the rule is harmless if it never fires.
- Remote "now": append `; echo __deck_now__ ; date +%s` to the `agent_probe`
  compound (`src/infra/tmux/remote.rs`) — same ssh hop as `ps`, and the
  remote's own clock sidesteps local/remote skew. Marker must not start with
  `=` or `-` (see CLAUDE.md's remote-shell traps). Local uses
  `SystemTime::now()`.
- Freshness: `now - window_activity <= 2 s` (two ticks) → pane is emitting.
- Trust the clock only when `window_panes == 1`.
- **Measure first**: an idle Claude with a custom statusLine may repaint on a
  timer and keep the clock warm. "Frozen ⇒ idle" always holds regardless, so
  the risk is only that the signal never triggers; if so, scope the clock to
  Working-retraction only.

### PR 3 — merge layer

The `merge` function above, table-driven tests, wired where local
(`src/system/tmux.rs`) and remote (`agent_probe`) currently assign
`a.status`. `prev` comes from `AppState.agents` (already keyed by `LaneId`).

### PR 4 — hook read side

- `PANE_FORMAT` gains `#{@deck_agent_state}` and `#{@deck_agent_session}`;
  parse into `PaneInfo`, feed `merge`.
- Transport is a **pane-scoped tmux user option** written by the hook script
  via the `$TMUX_PANE` the agent inherited: zero extra hops (read in the
  `list-panes` we already pay for, local and remote alike), no resident
  process on the remote, keyed by the `%N` pane id we already target, and
  gone automatically when the pane dies. Requires tmux ≥ 3.0; older servers
  read empty → path 4 never fires.
- Value format `state@epoch` (remote epoch, compared against the probe's
  `date +%s`). Working reports use a short TTL (~60 s — a killed agent never
  sends `Stop`); Idle/Blocked are stable states and may live longer.
- Production no-op until PR 5, but fully verifiable by hand-setting the
  option.

### PR 5 — hook install (the only step touching user config)

- Script `assets/agent-hooks/deck-agent-state.sh`, embedded via
  `include_str!`, version header `DECK_HOOK_VERSION=1`. Pure `/bin/sh` +
  one `tmux set-option` (`sed` extracts `session_id`; no python/jq — herdr
  needs python3 only because it speaks JSON over a socket). Guards: not in
  tmux → exit 0; `DECK_HOOKS=0` → exit 0; every step `|| :` then exit 0.
  Never writes stdout/stderr (`UserPromptSubmit` stdout is injected into the
  prompt; `SessionStart` stdout/stderr is shown to the user).
- Subscriptions, symmetric, **no tool events** (per the herdr analysis and
  the activity clock): `SessionStart` → session identity; `UserPromptSubmit`
  → `working@now`; `PermissionRequest` → `blocked@now`; `Stop` → `idle@now`;
  `SessionEnd` → clear (timeout 3 s on Codex; not a sole cleanup path — Codex
  skips it on SIGHUP). Hook timeout 5 s otherwise. Ignore `agent_id` payloads
  and `SubagentStop` unconditionally.
- Idempotence: own entries tagged `"_deck": true` (Otty's pattern) inside
  `~/.claude/settings.json` / `~/.codex/hooks.json`; managed script file
  beside them (herdr's pattern — reinstall overwrites the file, user hooks
  live untouched next to it). JSON merge happens **in deck** (ssh `cat` →
  merge in Rust with `serde_json` preserve_order → `printf` write-back) so
  the remote needs nothing installed, and it is **byte-stable**: an install
  over an already-correct file writes nothing (verified: second `deck hooks
  install` reports "unchanged" with zero writes), because any rewrite of
  `hooks.json` re-triggers the Codex trust prompt. Codex `[features]
  hooks = false` is **reported by `status`, never flipped** — silently
  re-enabling a mechanism the user turned off is exactly the move the trust
  gate exists to catch (deviation from herdr, which sets it to true). Skip
  silently when `~/.claude`/`~/.codex` is absent. **User-level config only —
  never the project-level `.claude/settings.json`** (it would be committed
  into the user's repo).
- **Timing (the rule the Codex trust gate imposes)**: install only on an
  explicit user action. As shipped that action is `deck hooks
  install [target]` (CLI paths exit before the instance guard); it reports
  per target/agent and ends with the trust notice: the next Codex launch on
  each touched machine asks once to trust deck's hooks. **Never as a silent
  side effect of attach or snapshot** — a security modal the user can't
  attribute is the one way this feature can betray them. A config flag +
  settings-UI toggle (auto-install when adding a lane while enabled) is
  deferred until the settings surface grows a row for it; the CLI is the
  consent surface for now. Container lanes are also deferred: their `$HOME`
  is volatile and the exec-wrapped path is untested — `RemoteFs` already
  goes through `run_ssh`, so they cost only testing when wanted.
- Version discipline: every `DECK_HOOK_VERSION` bump re-triggers the Codex
  trust modal on every machine. Bump only for behavior changes.
- Uninstall: remove our entries + script, leave everything else (herdr's
  uninstall likewise leaves `config.toml` unchanged).
- Local cache `~/.local/state/deck/agent-hooks.json` (lane + agent +
  version) so `status` answers without ssh.

### PR 6 — observability (shipped inside PR 5)

The trust gate makes "installed" ≠ "running". The hook writes
`@deck_hook_alive` on `SessionStart`, and `deck hooks status` prints a live
count of panes carrying it per target, so the three states separate: not
installed; installed and reporting; installed with entries current but zero
reporting panes while agents visibly run — waiting on Codex's trust
confirmation.

Verified end to end against a real Claude Code (2.1.241, sandboxed tmux):
`SessionStart` wrote the session UUID + alive mark, `UserPromptSubmit`/`Stop`
cycled working→idle, and `PermissionRequest` wrote `blocked@…` ~10 s before
the dialog was even painted. Exiting killed the pane and the options with it
— the third cleanup layer under `SessionEnd`-clear and the TTLs.

## Dependency order

```
PR1 (screen) ──┐
PR2 (activity) ─┼─> PR3 (merge) ─> PR4 (hook read) ─> PR5 (install) ─> PR6 (status)
```

PR1 ∥ PR2. Before PR2 lands, verify `window_activity` on more than one host
(TS-MINI zsh/mac, NUS-H200x4 linux) and inside a container lane, per
CLAUDE.md's remote-shell rule.

## Verification protocol (reusable)

Drive a sandboxed agent from the CLI, never inside the running deck:

```bash
mkdir -p /tmp/dt   # BEFORE anything else — see the trap below
env -u TMUX -u TMUX_PANE TMUX_TMPDIR=/tmp/dt \
    tmux new-session -d -s t1 -x 200 -y 50 "cd <trusted-dir> && exec claude"
# assert isolation actually took before driving anything:
env -u TMUX TMUX_TMPDIR=/tmp/dt tmux display -p '#{socket_path}'   # must print /tmp/dt/…
env -u TMUX TMUX_TMPDIR=/tmp/dt tmux send-keys -t t1 "..." Enter
env -u TMUX TMUX_TMPDIR=/tmp/dt tmux capture-pane -p -t t1
```

**The trap that makes the mkdir + assert mandatory**: if the directory
`TMUX_TMPDIR` points at does not exist, tmux (3.7c measured) does not error —
it silently falls back to the **default** socket, so every "sandboxed"
command lands on the real server: the test session appears among the user's
sessions, and a cleanup `kill-server` would kill their server. This happened
once (2026-08-24, an `rm -rf` of the tmpdir without re-mkdir); the
`#{socket_path}` assertion is what turns the mistake into a loud failure.

Claude needs the real `HOME` (credentials live in the macOS Keychain; a
sandboxed `HOME` cannot log in) — scope hooks via the throwaway project's
`.claude/settings.local.json`. Codex isolates cleanly via `CODEX_HOME` with
`auth.json` symlinked in. Logger hooks must write to a file only.
