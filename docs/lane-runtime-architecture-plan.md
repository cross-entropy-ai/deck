# Lane-centric runtime architecture execution plan

Status: approved for implementation on `feature/lane-runtime-architecture`.

Baseline: `main` at `v0.11.1` (`9994d2a`).

## Outcome

Deck becomes a shell over mounted lane runtimes. Local and remote are transport
details of the built-in tmux runtime, not application-level session variants.
The reducer, effect protocol, sidebar, focus policy, and attachment selection
address sessions through stable identities and capabilities without decoding a
host or branching on local versus remote.

```text
App / reducer / UI
        |
        v
SystemRegistry -> LaneRuntime
                    |-- SessionCatalog
                    |-- SessionController
                    |-- LaneController
                    `-- AttachmentProvider
                              |
                              v
                      AttachmentManager
                              |
                              v
                       TerminalSurface
                      (PTY + VT + OSC)
```

The built-in topology is:

```text
TmuxSystem
  |-- tmux/local lane  ---- local tmux control + local attach transport
  `-- tmux/<host> lane ---- SSH tmux control + SSH attach transport
```

## Architectural invariants

These are acceptance rules, not preferences.

1. `SessionId { lane: LaneId, key: String }` is the only application-level
   identity for a session. A display name is not a globally unique id.
2. `Session` and its UI DTOs do not carry `host`, `is_remote`, or transport
   enums. Backend-specific metadata stays behind a runtime interface.
3. App and reducers do not match on a system id or decode `LaneId::lane()`.
4. Session effects use one protocol for every lane. There are no paired
   `Foo`/`RemoteFoo` effect variants.
5. Snapshot reads may run concurrently. Mutating or display-affecting commands
   for one lane run in one FIFO, including session activation and pane focus.
6. Terminal bytes, VT parsing, OSC 10/11 replies, and OSC 52 forwarding belong
   to `TerminalSurface`. The host-terminal OSC 11 auto-theme probe remains a
   separate shell service because it interrogates Deck's parent terminal.
7. Interfaces are capability-sized. A backend that can list sessions is not
   forced to implement attachment, mutation, or lane actions.
8. Backend-specific behavior is exposed as typed capabilities/actions; it does
   not add backend-specific branches to App or `Effect`.
9. A failed worker spawn, command submission, backend call, or attachment
   transition becomes a typed visible failure. It is never represented as
   unreachable data, a permanent connecting state, or a dropped intent.
10. Every migration step leaves the workspace compiling and tests passing.

## Target domain and ports

Names may be adjusted to match Rust conventions, but responsibility and
dependency direction must remain the same.

```rust
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SessionId {
    pub lane: LaneId,
    pub key: String,
}

pub struct Session {
    pub id: SessionId,
    pub name: String,
    pub directory: Option<String>,
    pub state: SessionState,
    pub capabilities: SessionCapabilities,
}

pub trait SessionCatalog: Send + Sync {
    fn snapshot(&self, lane: &LaneId, ctx: &SnapshotCtx<'_>)
        -> Result<LaneSnapshot, CatalogError>;
}

pub trait SessionController: Send {
    fn execute(&self, command: SessionCommand) -> SessionControlResult;
}

pub trait LaneController: Send {
    fn execute(&self, command: LaneCommand) -> LaneControlResult;
}

pub trait AttachmentProvider: Send + Sync {
    fn connect(&self, lane: &LaneId, size: TerminalSize)
        -> Result<TerminalSurface, AttachmentError>;
}
```

The application effect protocol converges on:

```rust
Effect::ActivateSession(SessionId)
Effect::ExecuteSession {
    target: SessionId,
    command: SessionCommand,
}
Effect::ExecuteLane {
    lane: LaneId,
    command: LaneCommand,
}
Effect::InvokeLaneAction {
    lane: LaneId,
    action: LaneActionId,
    anchor: Option<Point>,
}
```

`SessionCommand` initially covers rename and kill. Creation, ordering, directory
listing, reconnect, remove-lane, and forward management are lane operations.
This prevents artificial "session" methods that have no session target.

## Work packages

### WP0 — Lock the architecture with contract tests

- [ ] Add compile/runtime fixture coverage for a second, non-tmux system.
- [ ] Exercise the fixture through App-facing effect routing, attachment
      selection, activation, snapshot, and one lane action—not registry lookup
      alone.
- [ ] Add source-architecture guards that reject new local/remote effect pairs
      and host fields in generic session DTOs once their migrations land.
- [ ] Record the current live tmux/SSH smoke-test procedure without claiming it
      ran in CI.

Exit: the fixture demonstrates the desired dependency boundary and initially
fails only at the compatibility seams targeted below.

### WP1 — Stable session identity and unified effects

- [ ] Introduce `SessionId` and migrate selection, agent targets, focus
      bookkeeping, request DTOs, and session lookup to it.
- [x] Replace `SwitchSession` and `SwitchRemote` with `ActivateSession`.
- [ ] Replace local/remote picker and order effects with lane-keyed commands.
- [ ] Replace host-bearing kill/rename/create routing with `SessionId` or
      `LaneId` as appropriate.
- [ ] Keep config serialization compatibility through adapter functions at the
      tmux/config boundary.
- [ ] Delete compatibility accessors after all callers migrate.

Exit: reducers and `effect_runner` do not branch on local/remote for session or
lane commands; existing config files still load unchanged.

### WP2 — Per-lane command serialization and typed failure semantics

- [ ] Generalize `SessionExecutor` into a per-lane command executor or add a
      sibling executor shared by activation and pane focus.
- [ ] Serialize all state-changing/display-changing work for the same lane:
      activate, focus pane, rename, kill, create, reorder, and reconnect-sensitive
      control calls.
- [ ] Keep different lanes independent and keep snapshots parallel.
- [ ] Remove the thread-per-click `FocusExecutor` write path.
- [ ] Reject stale results before a side effect when possible; sequence ids may
      still protect UI reconciliation but must not be the only race defense.
- [ ] Return typed outcomes for worker spawn failure, send failure, panic,
      timeout, backend failure, and stale generation.

Exit: a deterministic test proves a slow older focus/activate command cannot
overwrite a newer command in the same lane; commands on two lanes can progress
independently.

### WP3 — Split the System interface into narrow runtime ports

- [ ] Extract catalog, session-control, lane-control, lane-action, and
      attachment-provider ports.
- [ ] Compose them in a `LaneRuntime` returned/resolved by the registry.
- [ ] Keep `System` responsible only for configuration and lane enumeration, or
      replace it with an equivalent composition-root abstraction.
- [ ] Change snapshot failure from `Option<LaneSnapshot>` to a typed result so
      internal worker failure is not rendered as network unreachability.
- [ ] Express lane/session capabilities in model data and disable/hide
      unsupported UI actions from capabilities.

Exit: adding a backend with only catalog + activation capabilities requires no
dummy implementations and no App changes.

### WP4 — Lane-keyed attachment manager

- [ ] Introduce `AttachmentManager` keyed by `LaneId`.
- [ ] Model each lane as `Disconnected`, `Connecting`, `Connected(surface)`, or
      `Failed(error)` with a monotonically increasing generation.
- [ ] Migrate `App.local_terminal` and `RemoteConnManager` terminal ownership
      into the manager while retaining tmux-specific providers in adapters.
- [ ] Route activate, reconnect, resize, input, render, and recovery through
      the active lane instead of host/local branches.
- [ ] Make spawner failure transition to `Failed`; never leave a lane stuck in
      `Connecting`.
- [ ] Preserve lazy remote connection and the always-available local lane as
      policy configured at composition time, not hard-coded fields in App.

Exit: App owns one attachment manager and has no `local_terminal` or `remote`
terminal fields; switching display lanes is lane-keyed.

### WP5 — TerminalSurface and OSC boundary

- [ ] Rename/extract `TerminalPane` as `TerminalSurface` in a terminal module.
- [ ] Keep PTY read/write/resize, vt100 parsing, OSC 10/11 reply generation,
      and OSC 52 forwarding in this boundary.
- [ ] Make OSC forwarding conditional on the active `TerminalSurface` rather
      than local/remote origin.
- [ ] Keep parent-terminal OSC 11 auto-theme probing in the shell lifecycle and
      document the distinction.
- [ ] Add parser tests for split/multiple OSC sequences and inactive-surface
      clipboard suppression.

Exit: OSC behavior is identical for any attachment provider and no Session API
mentions OSC.

### WP6 — Backend actions and compatibility-seam removal

- [ ] Replace `ReconnectHost`, `OpenForwardOverlay`, `RemoveRemoteHost`, and
      host-based divider menu effects with lane actions/capabilities.
- [ ] Move tmux/SSH-specific action interpretation entirely into its runtime
      adapter, returning generic shell intents only where UI is required.
- [ ] Remove `SessionEntry.host`, `SectionDef.runtime_key`, host-keyed runtime
      maps, and `Option<String>` local/remote sentinels from generic layers.
- [ ] Update config adapters, documentation, module comments, and naming.
- [ ] Delete dead compatibility code only after `rg` proves it has no callers.

Exit: the only local/remote and SSH/tmux distinctions live under the built-in
tmux/attachment adapters and configuration translation.

## Delivery sequence

Use small, reviewable commits in this order:

1. `test: define lane runtime architecture contract`
2. `refactor: introduce stable session identities`
3. `refactor: unify lane and session effects`
4. `fix: serialize display commands per lane`
5. `refactor: split lane runtime interfaces`
6. `refactor: unify terminal attachment management`
7. `refactor: isolate terminal surface and OSC handling`
8. `refactor: remove tmux attachment compatibility seams`
9. `docs: finalize lane runtime architecture`

Before each commit:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
git diff --check
```

Run targeted tests during development; run the complete gate above at every
work-package exit and before publication.

## Manual acceptance matrix

Automated tests do not replace these runtime checks. Record unexecuted rows as
such in the PR.

| Scenario | Expected result |
|---|---|
| Local tmux list/activate/focus/rename/create/kill/reorder | FIFO behavior and immediate visible reconciliation |
| Remote zsh host | Attach and every session operation succeed without shell-expansion errors |
| Remote bash host | Same behavior as zsh host |
| Slow remote, then immediate local selection | Newer selection remains active; stale remote work cannot clobber it |
| Two rapid focus requests in one lane | Execution order is deterministic and the final target is the newest request |
| Remote spawn failure | Lane enters visible `Failed`, retry works, UI remains responsive |
| OSC 10/11 from active local and remote surfaces | Correct color reply reaches the child |
| OSC 52 from active/inactive surfaces | Only active surface reaches the parent terminal clipboard |
| Auto theme OSC 11 probe | Still follows the parent terminal independently of child OSC handling |
| Existing `~/.config/deck/config.yaml` | Loads without migration loss and saves compatible values |

## Risk controls and rollback

- Preserve behavior before deleting compatibility paths; prefer adapters over a
  flag day rewrite.
- Do not change persisted config shape in the same commit as runtime routing.
- Keep each work package independently revertible and never commit a knowingly
  failing workspace.
- Attachment and command state transitions must carry generation ids so late
  results cannot revive superseded connections.
- Avoid new global registries or stringly typed action names. New public ids and
  commands must be typed, hashable where needed, and covered by equality tests.
- No release is part of this plan. Merge and release require explicit approval
  after CI, review, and the applicable manual smoke tests.

## Definition of done

- [ ] All architectural invariants hold in production code.
- [ ] The non-tmux fixture crosses the full App-to-terminal contract without
      editing App, reducer variants, or renderer.
- [ ] No generic module branches on local/remote or carries a host sentinel.
- [ ] Same-lane display mutations are FIFO and failure-complete.
- [ ] OSC behavior is surface-scoped and covered for local/remote-independent
      paths.
- [ ] Workspace gates pass.
- [ ] Architecture docs describe the implemented state, not the intended one.
- [ ] PR lists manual smoke-test evidence and explicitly identifies unrun rows.
