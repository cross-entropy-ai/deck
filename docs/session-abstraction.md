# Session control abstraction

Status: implemented for the control plane.

Local and remote session mutations share one typed interface and one async
execution model. Transport differences remain in leaf adapters; App submits a
lane plus an operation and consumes a typed outcome.

## Scope

`SessionControl` covers operations that can block and must not run on the UI
thread:

- switch;
- rename;
- kill;
- create;
- persist display order;
- list directories for the new-session picker.

PTY creation, resize, drain/write, SSH reconnect state, and attachment recovery
are lifecycle services, not session-control methods. Keeping that distinction
avoids a broad interface whose implementations would depend on unrelated UI
runtime state.

## Contract

```rust
pub trait SessionControl {
    fn switch_to(&self, name: &str) -> SessionControlResult;
    fn rename(&self, old: &str, new: &str) -> SessionControlResult;
    fn kill(&self, name: &str) -> SessionControlResult;
    fn create(&self, name: &str, dir: &str) -> SessionControlResult;
    fn persist_order(&self, order: &[String]) -> SessionControlResult;
    fn list_dir(&self, path: &str) -> SessionControlResult<DirListing>;
}
```

All methods return `SessionControlResult<T>`. A backend failure is never
encoded as `false`, an empty tuple, or a successful executor outcome.
`DirListing` is a named DTO, leaving room to extend directory metadata without
changing tuple conventions.

## Execution and causality

`SessionExecutor` owns one FIFO worker per `LaneId`. Local and remote calls
therefore have identical scheduling and error semantics, while operations for
the same lane retain submission order. Outcomes return on a channel and are
applied on the UI thread.

```text
Effect -> App::submit_session
       -> registry.for_lane(lane)
       -> System::control(lane, ControlCtx)
       -> per-lane FIFO worker
       -> OpOutcome::{Created, Renamed, Killed, DirListed, Switched,
                      OrderPersisted, Failed}
       -> App reconciliation
```

`Created` exists only after successful creation. Every operation failure maps
to `OpOutcome::Failed { operation, error }`, which surfaces an in-UI warning and
requests reconciliation. Directory-list failures retain the requested path so
a stale picker response can still be rejected deterministically.

Submission is fallible too: `SessionExecutor::submit` returns a typed error if
its lane worker cannot start or accept the job. A backend panic is caught and
translated into the same visible failure protocol; the worker remains alive to
process later FIFO jobs. Neither boundary silently drops user intent.

## Backend boundary

- `LocalControl` delegates to bounded local tmux helpers and filesystem reads.
- `RemoteControl` captures the remote host and connection generation, then
  delegates to bounded SSH/tmux helpers.
- `ControlCtx` is backend-neutral: it carries an opaque local client locator
  and a `LaneId -> generation` map. App does not construct tmux controls
  directly.

The nesting guard remains App policy because only Deck's local enclosing tmux
session can recursively contain Deck itself. Post-create switching, kill
pre-switch behavior, focus reconciliation, and attachment recovery also remain
orchestration policies rather than backend primitives.
