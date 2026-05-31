# Refresh-Button Color + Add Remote Host — Design

Date: 2026-05-31
Status: Draft

Two independent sidebar/remote-host features shipped together (one design, one
plan, one PR). Branch is cut from `main`; an open PR (#55, port-forward status)
also reworks `render_group_header`, so Part 1 here will need a trivial one-line
reconcile if that merges first.

## Part 1 — Refresh button defaults to the divider color

### Summary

On each remote host divider (`@host ───── [⟳] […]`), the reconnect glyph `[⟳]`
is currently tinted `theme.green` when the host is Connected, which clashes with
the rest of the divider (label, rule, `[…]`) that all use the per-host `accent`
color. Make `[⟳]` use `accent` when Connected so the divider reads as one unit.
Keep the warning colors for the non-healthy states.

### Change

In `src/ui/sidebar.rs::render_group_header`, the `reconnect_fg` match:

```rust
let reconnect_fg = match status {
    HostStatus::Connected => accent,        // was theme.green
    HostStatus::Connecting => theme.yellow, // unchanged
    HostStatus::Unreachable => theme.pink,  // unchanged
};
```

`accent` is already a parameter (it tints the label/rule/`[…]`). This is a
one-line change. Connecting (yellow) and Unreachable (pink) keep their warning
tint so connection trouble is still visible at a glance.

### Testing

Unit test on `render_group_header`: render with each `HostStatus` and assert the
`[⟳]` span's foreground — `accent` for Connected, `theme.yellow` for Connecting,
`theme.pink` for Unreachable. (Inspect the pushed `Line`'s spans for the one
whose content is `[⟳]`.)

## Part 2 — Add Remote Host from the context menu

### Summary

Add an `Add Remote Host` item to the global right-click menu, right after
`New session`. Selecting it opens a centered popup that lists the hosts from
`~/.ssh/config` (minus ones already added) and also accepts a freely-typed
hostname. Confirming adds the host to `config.remotes`, persists `config.json`,
and triggers the existing onboarding + refresh so the host's sessions appear
without a restart.

### Goals

- Add a remote host from inside the TUI, no CLI / restart.
- Offer `~/.ssh/config` hosts as suggestions; allow any typed hostname too.
- Reuse the existing add-host onboarding (`onboard_remote_host`) and overlay/
  picker patterns; keep `state.rs` / `sidebar.rs` from growing.

### Non-goals

- Following `Include` directives in `~/.ssh/config` (MVP parses the main file;
  manual input covers hosts that live in included files).
- The SSH ControlMaster-multiplexing setup / `~/.ssh/config` snippet append the
  CLI `deck add` does. The picker only adds the host to deck's list.
- Editing or reordering existing remotes (use the divider `[…]` → Remove).
- Mouse interaction inside the picker (keyboard-driven, like the existing
  new-session picker).

### Architecture

```
src/
  infra/ssh.rs            MODIFY — parse_config_hosts(content) (pure) +
                                   config_hosts() (reads ~/.ssh/config)
  model/
    add_remote.rs         NEW — AddRemoteState + filter helper (mirrors
                                 model/new_session.rs)
    state.rs              MODIFY — GLOBAL_MENU_ITEMS gains "Add Remote Host";
                                   OverlayState.add_remote; SideEffect
                                   .open_add_remote_picker + .add_remote_host
  ui/
    add_remote.rs         NEW — draw_add_remote popup (mirrors ui/new_session.rs)
    mod.rs / render.rs    MODIFY — render the picker when open
  app/
    action/
      mod.rs              MODIFY — AddRemote* actions
      keyboard.rs         MODIFY — route keys to the picker when open
      reduce.rs           MODIFY — menu "Add Remote Host" arm; AddRemote*
                                   reducer arms (filter, confirm, close)
    dispatch.rs           MODIFY — open_add_remote_picker → build candidates +
                                   open; add_remote_host → onboard_remote_host

tests/unit/
  infra/ssh.rs            MODIFY — parse_config_hosts cases
  model/add_remote.rs     NEW — filter + confirm-resolution + duplicate guard
  ui/sidebar.rs           MODIFY — reconnect_fg color per status (Part 1)
```

`model/add_remote.rs` and `ui/add_remote.rs` mirror the existing
`new_session` picker split (state+logic vs render), so the picker is a
self-contained unit and the big files don't grow.

### Data model

```rust
// model/add_remote.rs
pub struct AddRemoteState {
    /// Doubles as a live filter over `hosts` and a free-text hostname.
    pub input: TextArea<'static>,
    /// ~/.ssh/config candidates minus hosts already in config.remotes.
    /// Set by dispatch when the picker opens; reducer never refills it.
    pub hosts: Vec<String>,
    /// Indices into `hosts` whose name contains the input (case-insensitive).
    /// Recomputed by the reducer on every input change.
    pub filtered: Vec<usize>,
    /// Index into `filtered`; reducer clamps to `0..filtered.len()`.
    pub selected: usize,
    /// Last error (e.g. empty / already-added). Cleared on next mutation.
    pub error: Option<String>,
}
```

`OverlayState` gains `pub add_remote: Option<AddRemoteState>`.
`SideEffect` gains `pub open_add_remote_picker: bool` and
`pub add_remote_host: Option<String>` (parallel to `remove_remote_host`).

### SSH host enumeration (`infra/ssh.rs`)

```rust
/// Parse `~/.ssh/config` text into the list of concrete Host aliases.
/// Splits each `Host` line into tokens, drops wildcard/negation patterns
/// (containing `*`, `?`, or `!`), de-dups, preserves first-seen order.
pub fn parse_config_hosts(content: &str) -> Vec<String>;

/// Read `~/.ssh/config` and return `parse_config_hosts`. Missing/unreadable
/// file → empty Vec (the picker still accepts typed input).
pub fn config_hosts() -> Vec<String>;
```

Parsing rule: for each line whose first token (case-insensitive) is `Host`,
take the remaining whitespace-separated tokens; keep those without `*`, `?`,
`!`; skip blanks/comments. (Effective per-host options are out of scope — the
picker only needs the alias to add to deck and later resolve via `ssh -G`.)

### UI (`ui/add_remote.rs`)

Centered popup, `popup_frame` + `centered_rect`, `theme.surface` background —
same chrome as the rename / port-forward overlays.

```
┌─ Add Remote Host ─────────────────────────┐
│                                            │
│  host: [prod-web________]                  │   input: filter / any hostname
│                                            │
│  > prod-web-1                              │   filtered ~/.ssh/config hosts
│    prod-web-2                              │   (">" marks `selected`)
│    staging                                 │
│                                            │
│  err: already added                        │   (only when error is set)
│  [↑↓] select   [enter] add   [esc] cancel  │
└────────────────────────────────────────────┘
```

Empty candidate list (no ssh config / all already added): show
`(no ~/.ssh/config hosts — type a hostname)` in the list region.

### Behavior

| Key            | Action                                                        |
| -------------- | ------------------------------------------------------------- |
| char / backspace | edit `input`; reducer recomputes `filtered`, clamps `selected` |
| ↑ / ↓          | move `selected` within `filtered`                             |
| enter          | confirm (see resolution rule)                                 |
| esc            | close picker, no change                                       |

**Confirm resolution:** if `filtered` is non-empty → chosen =
`hosts[filtered[selected]]` (the highlighted row). If `filtered` is empty →
chosen = trimmed `input` (a literal hostname). Then validate:

- chosen is empty → `error = "enter a hostname"`, stay open.
- chosen already in `config_remotes` → `error = "already added"`, stay open.
- otherwise → add it (below).

(Trade-off: when the typed text is a substring of a listed host, enter adds the
highlighted match, not the literal. To add an unlisted host, type its full name
until the list empties. Acceptable for MVP.)

### Add flow

Mirrors `RemoveRemoteFromList`, additive. The reducer's `AddRemoteConfirm` arm,
on a valid choice:

```rust
state.config_remotes.push(RemoteConfig { host: host.clone(), forwards: vec![] });
state.overlay.add_remote = None;
fx.save_config = true;
fx.refresh_sessions = true;
fx.add_remote_host = Some(host);
```

Dispatch consumes `fx.add_remote_host` by calling the existing
`self.onboard_remote_host(&host)` — the same path `reload_config` uses for a
newly-added host (seeds runtime state + spawns the PTY so selecting the new
section connects). `fx.refresh_sessions` then surfaces its sessions.

The 2-second config-poll fires `ReloadConfig` after the save; its old/new diff
finds the host already in both (we added it in-memory and to disk), so it
re-onboards nothing — idempotent.

### Open flow

Reducer's menu `MenuConfirm` Global arm:
`Some("Add Remote Host") => SideEffect { open_add_remote_picker: true, .. }`.

Dispatch on `fx.open_add_remote_picker`:

```rust
let existing: HashSet<&str> = self.state.config_remotes.iter()
    .map(|r| r.host.as_str()).collect();
let hosts: Vec<String> = crate::infra::ssh::config_hosts()
    .into_iter().filter(|h| !existing.contains(h.as_str())).collect();
self.state.overlay.add_remote = Some(AddRemoteState::new(hosts));
```

`AddRemoteState::new(hosts)` seeds `filtered = 0..hosts.len()`, `selected = 0`.

### Error handling

| Scenario                              | Behavior                                                   |
| ------------------------------------- | ---------------------------------------------------------- |
| `~/.ssh/config` missing/unreadable    | candidate list empty; typed input still works.             |
| Empty hostname on confirm             | `err: enter a hostname`; picker stays open.                |
| Host already in `config_remotes`      | `err: already added`; picker stays open.                   |
| Host unreachable after add            | onboarding marks it Connecting→Unreachable like any host; the divider shows it (pink `[⟳]`). No special handling. |

### Testing

- `parse_config_hosts`: sample config with multiple `Host` lines, a
  multi-token `Host a b`, a `Host *` wildcard, a `Host foo?bar` pattern, a
  comment/blank line → expected concrete aliases (wildcards/patterns excluded,
  order preserved, de-duped).
- `AddRemoteState` filter: input → expected `filtered` indices (substring,
  case-insensitive); `selected` clamps when the list shrinks.
- Confirm resolution: non-empty filtered → highlighted host; empty filtered →
  trimmed input; empty input → error; duplicate → error. (Pure helper over
  state + the existing `config_remotes` host set.)
- Part 1: `render_group_header` `[⟳]` fg per `HostStatus`.

Manual: right-click → Add Remote Host → pick a config host → it appears and
connects; type an unlisted hostname → added; try an already-added host → error.
