# Port Forward for Remote Hosts — Design

Date: 2026-05-28
Status: Draft

## Summary

Add per-host SSH port-forward configuration to deck. The remote-session divider in
the sidebar gains a trailing `[…]` button. Clicking it opens a one-item menu
(`Port Forward`) which leads to an overlay where the user manages forwards
(Local `-L`, Remote `-R`, Dynamic `-D`). Forwards are persisted to
`~/.config/deck/config.json` and applied automatically at deck startup
(eager). Changes made through the overlay take effect immediately on the live
ControlMaster.

## Goals

- Configure SSH port forwards per remote host from inside deck.
- Persist forwards and apply them at every deck startup without user action.
- Apply add/delete during a deck session immediately (no restart required).
- Stay within the existing module boundaries and overlay/menu patterns.

## Non-goals

- Editing forwards in place (delete + re-add covers MVP).
- Enabled/disabled per-forward toggle (use delete instead).
- Tunneling to hosts not already configured as remotes in deck.
- Killing the ControlMaster on deck exit (rely on `ControlPersist=10m`).
- Hover styling for the `[…]` button.

## Architecture

```
src/
  infra/
    port_forward.rs        NEW — pure helpers: build_master_cmd, build_forward_cmd,
                                  build_cancel_cmd, build_exit_cmd; flag formatting
  model/
    config.rs              MODIFY — RemoteConfig.forwards, ForwardSpec, ForwardMode
    state.rs               MODIFY — PortForwardOverlay, PfAddForm, PfField,
                                    MenuKind::HostDivider, DividerHit map
  ui/
    sidebar.rs             MODIFY — render_group_header emits `[…]` and returns hit rect
    overlays/
      port_forward.rs      NEW — list overlay + add subform rendering
  app/
    action/
      mouse.rs             MODIFY — divider button hit-test → OpenHostDividerMenu
      reduce.rs            MODIFY — OpenPortForward, PfAdd*, PfDelete, PfClose,
                                    PfTaskResult, ReloadConfig diff for forwards
    port_forward_task.rs   NEW — worker thread + command channel; manages master
                                  lifecycle and forward operations
  main.rs                  MODIFY — spawn port_forward_task on startup with
                                    config.remotes

tests/unit/
  infra/port_forward.rs    NEW — ssh command construction for all modes/options
  model/config.rs          MODIFY — forwards (de)serialization, legacy compat
```

### Layering rationale

- `infra/port_forward.rs` holds pure `Command` builders and flag formatting; no
  IO, no threads. Matches the role of `infra/remote_tmux.rs`.
- `app/port_forward_task.rs` owns the worker thread and channel. Matches the
  role of `app/remote_spawn.rs` (lifecycle on top of an `infra` builder).
- UI is split into the divider button (in `ui/sidebar.rs`) and the overlay
  proper (`ui/overlays/port_forward.rs`), keeping `sidebar.rs` from growing.

## Data Model

### Persisted config

```rust
// model/config.rs
#[derive(Serialize, Deserialize, Clone)]
pub struct RemoteConfig {
    pub host: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub forwards: Vec<ForwardSpec>,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct ForwardSpec {
    pub mode: ForwardMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bind_addr: Option<String>,
    pub listen_port: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_port: Option<u16>,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ForwardMode { Local, Remote, Dynamic }
```

`#[serde(default)]` on `forwards` makes pre-feature config files load with an
empty vector. `skip_serializing_if = "Vec::is_empty"` keeps new writes clean.

Example:

```json
{
  "remotes": [{
    "host": "server-1",
    "forwards": [
      { "mode": "local",   "listen_port": 8080,
        "target_host": "localhost", "target_port": 80 },
      { "mode": "dynamic", "listen_port": 1080 }
    ]
  }]
}
```

### Validation invariants

- `Local`/`Remote`: `target_host` and `target_port` must be `Some`.
- `Dynamic`: `target_host` and `target_port` must be `None` (loader strips them
  with a warning if present, rather than failing).
- `listen_port` in `1..=65535`. `target_port` likewise when present.
- Invalid entries are dropped at load time with a logged warning. The rest of
  the config still loads.

### SSH flag projection

| Mode    | flag                                                            |
| ------- | --------------------------------------------------------------- |
| Local   | `-L [bind_addr:]listen_port:target_host:target_port`            |
| Remote  | `-R [bind_addr:]listen_port:target_host:target_port`            |
| Dynamic | `-D [bind_addr:]listen_port`                                    |

Implemented as `ForwardSpec::to_ssh_flag(&self) -> String`. Bind addr is
omitted from the flag when `None`.

### Runtime state

```rust
// model/state.rs
pub struct PortForwardOverlay {
    pub host: String,
    pub selected: usize,
    pub add_form: Option<PfAddForm>,
    pub status: Option<String>,
}

pub struct PfAddForm {
    pub mode: ForwardMode,
    pub focus: PfField,
    pub bind_addr: String,
    pub listen_port: String,
    pub target_host: String,
    pub target_port: String,
}

pub enum PfField { Mode, BindAddr, ListenPort, TargetHost, TargetPort }

pub enum MenuKind {
    Session    { target: FocusTarget, items: &'static [&'static str] },
    HostDivider { host: String,        items: &'static [&'static str] }, // NEW
}
const HOST_DIVIDER_MENU_ITEMS: &[&str] = &["Port Forward"];
```

`OverlayState` gains `pub port_forward: Option<PortForwardOverlay>`.

A `DividerHit { host: String, button_rect: Rect }` collection is produced by
the sidebar renderer and stored in `AppState::sidebar_hits.dividers` (new
field). Mouse handling reads it.

## UI

### Divider button

`render_group_header` emits the existing `label + dashes` then a trailing
`[…]` (3 cells) in the same accent color as the rest of the divider. The
function returns a `DividerHit` with the `[…]` rect so the caller can register
it for hit-testing.

```
 server-1 ───────────────────── […]
```

Dashes shrink by 4 cells (3 for `[…]`, 1 spacer) so total width stays
constant.

### Trigger paths

| Input                                            | Action                                |
| ------------------------------------------------ | ------------------------------------- |
| Mouse click on `[…]` rect of host H              | `OpenHostDividerMenu { host: H, x, y }` → `MenuKind::HostDivider` near button |
| Keyboard `f` while focus is on any remote session of host H | `OpenPortForward(H)` (skips the menu) |
| Menu click on `Port Forward`                     | `OpenPortForward(host)`               |

`f` is bound only when the focused row is a remote session; local-session
focus leaves `f` free for future use.

### Main overlay (forward list)

Centered modal, width ~64 cols, height = `4 + forwards.len() + 3`.

```
┌─ Port Forward — server-1 ─────────────────────────────┐
│                                                       │
│  > -L localhost:8080  → localhost:80                  │
│    -D 1080                                            │
│    -R 0.0.0.0:9090    → localhost:5432                │
│                                                       │
│  status: forward applied                              │
│                                                       │
│  [a] add   [d] delete   [esc] close                   │
└───────────────────────────────────────────────────────┘
```

Keys:
- `↑`/`↓` or `k`/`j`: move selection
- `a`: open add subform
- `d`: delete selected (after task confirms cancel)
- `esc`: close overlay

Empty state body: `(no forwards configured — press a to add)`.

### Add subform

Replaces the list region in place (no nested modal). Pressing `esc` or `enter`
returns to the list.

```
┌─ Port Forward — server-1  ▸ add ──────────────────────┐
│                                                       │
│  mode:        ( ) local   (•) remote   ( ) dynamic    │
│                                                       │
│  bind addr:   [_______________]   (optional)          │
│  listen port: [____]                                  │
│  target host: [_____________________]                 │
│  target port: [____]                                  │
│                                                       │
│  err: listen_port must be 1-65535                     │
│                                                       │
│  [tab] next   [enter] save   [esc] cancel             │
└───────────────────────────────────────────────────────┘
```

Behavior:
- Default `mode = Local`.
- `tab` / `shift-tab` cycles fields. Mode row uses `←`/`→`.
- When `mode = Dynamic`, target host and target port rows are dimmed and
  skipped by tab.
- `enter`: validate → dispatch `AddForward` task command → on success, append
  to config and return to list. On failure, keep the form and show the error.

Overlay visual style matches existing `Rename`/`Kill confirm` overlays:
rounded border, `theme.surface` background.

## Lifecycle

### Startup

`main.rs` spawns the port-forward worker after config load:

```
port_forward_task::spawn(config.remotes.clone(), ui_tx)
```

The worker, on its thread:

1. For each host with non-empty `forwards`:
   1. `ssh -fN -o ControlPersist=10m <host>` — backgrounds and exits as soon
      as the master is ready. The existing `ControlPath` from
      `remote_spawn.rs` (`~/.ssh/cm-%r@%h:%p`) is honored via user's ssh
      config or explicit `-o ControlPath=…` flag (see Open Question 1).
   2. For each `fwd` in that host's `forwards`: `ssh -O forward <flag> <host>`.
2. Enter the command loop on the channel.

`ssh -fN` returns immediately, so deck startup is not blocked. If a master
fails to come up, the host is flagged `unreachable` in sidebar state (reuse
existing field) and an error event is logged. The worker continues serving
other hosts and runtime commands.

### Runtime operations

UI dispatches Actions; the reducer sends commands on the worker channel:

| User action           | Worker command                         | ssh                                 |
| --------------------- | -------------------------------------- | ----------------------------------- |
| Add subform `enter`   | `AddForward { host, spec }`            | `ssh -fN host` (if no master yet) then `ssh -O forward <flag> host` |
| List `d`              | `CancelForward { host, spec }`         | `ssh -O cancel <flag> host`         |

Worker results come back as `Action::PfTaskResult { host, op, ok, message }`
fed into the reducer. The reducer:

- Updates `overlay.status`.
- On `AddForward` success: appends spec to `config.remotes[host].forwards`
  and saves config.
- On `AddForward` failure: keeps the add subform open, sets `status` to
  the error message, does not modify config.
- On `CancelForward`: removes the spec from config regardless of `ok` (avoid
  ghost entries; ssh exit may already be gone). Reports failures via
  `status`.

### Exit

deck exit does not stop the master. `ControlPersist=10m` lets the master
expire naturally. The next deck startup is idempotent: `ssh -fN host`
no-ops if the master is up, then `ssh -O forward` re-applies forwards. SSH
deduplicates identical forward specs on the same master, so re-application is
safe.

### Hot-reload integration

The existing config hot-reload (commit 84ab97d) fires `Action::ReloadConfig`
when `config.json` mtime changes. The reducer diffs old vs new `forwards`
per host:

- New spec → `AddForward`
- Removed spec → `CancelForward`
- Host removed entirely → `StopHostMaster` → `ssh -O exit <host>`
- Host added with forwards → bootstrap path (master + forwards)

Manual config edits and UI edits converge on the same worker commands.

## Error handling

| Scenario                                          | Behavior                                                                                              |
| ------------------------------------------------- | ----------------------------------------------------------------------------------------------------- |
| Startup: master cannot come up (host unreachable) | Host gets `unreachable` flag, other hosts proceed, error logged. Worker stays alive.                  |
| `ssh -O forward` reports port already in use      | Overlay status shows `port N already in use`; spec is not persisted; add form stays open.             |
| `ssh -O cancel` fails                             | Spec is still removed from config (avoid ghost entries); warning shown in status.                     |
| Form field invalid (port out of range, missing target for Local/Remote) | `enter` triggers local validation, red `err:` line; no worker command sent. |
| Config contains an invalid spec (hand-edited)     | Loader logs a warning and drops just that spec; rest of config loads.                                 |
| Worker thread panics                              | Logged; UI displays a one-shot toast; remaining session is degraded (no forward ops) but deck stays usable. |

## Testing

Pure unit tests:

- `ForwardSpec::to_ssh_flag()` for all three modes × `{bind_addr present, absent}`.
- `RemoteConfig` (de)serialization roundtrip. Legacy config without `forwards`
  loads with empty vec. New writes omit empty `forwards`.
- `PfAddForm::validate() -> Result<ForwardSpec, FormError>` boundary cases:
  port 0, 65535, 65536, missing target_host on Local, target fields set on
  Dynamic (cleared), non-numeric port.
- `diff_forwards(old: &[ForwardSpec], new: &[ForwardSpec]) -> Vec<ForwardOp>`
  for hot-reload.

Lightweight integration:

- `build_master_cmd(host) -> Command`, `build_forward_cmd(host, spec)`,
  `build_cancel_cmd(host, spec)`, `build_exit_cmd(host)`: assert constructed
  args.
- Worker logic via injected `Runner` trait stub: assert that
  `AddForward` issues master cmd before forward cmd if master not yet
  marked up; subsequent `AddForward` for the same host does not re-issue
  the master cmd.

Manual:

- Real SSH-reachable host: configure `-L 8080:localhost:80`, hit
  `localhost:8080`, verify response.
- Edit `config.json` externally to add/remove a forward; verify hot-reload
  applies it.
- Kill the master externally; verify next add via UI restarts it.

## Open questions

1. **ControlPath ownership.** `remote_spawn.rs` currently sets `-o
   ControlPath=~/.ssh/cm-%r@%h:%p` on every ssh invocation. The port-forward
   worker should pass the same `-o ControlPath=…` to guarantee both code
   paths talk to the same master. Decision: extract the ssh-options block
   into a shared helper used by both `remote_spawn.rs` and
   `infra/port_forward.rs`.

2. **`f` key conflict.** Need to grep current keymap to confirm `f` is
   unbound on remote session focus. If conflict, fall back to a less-loaded
   key (`F` or chord like `g f`).

## Risks

- **State drift between config and live ssh master.** If the user edits
  config externally while master is up and one of the new forwards collides
  with an existing one, ssh reports an error. Mitigation: hot-reload error
  is surfaced as a toast (new minor UI element) so user knows config write
  did not fully apply.
- **Slow startup for many hosts.** Each `ssh -fN` involves a TCP+SSH
  handshake. Mitigation: worker iterates hosts sequentially but each call
  returns quickly thanks to `-f`; no UI block.
- **Stale dashes calculation.** `[…]` taking trailing cells means narrow
  sidebars (~12 cols) lose most of the dash run. Acceptable; existing
  narrow-sidebar handling already truncates.
