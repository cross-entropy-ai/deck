# System architecture: deck as a mounted shell

Status: implemented.

deck's application shell mounts one or more `System` implementations. The
built-in `TmuxSystem` supplies the local tmux server plus configured remote
servers; a second backend participates by implementing the trait and being
added to the composition-root registry.

## Responsibility boundary

- The shell owns terminal panes, event-loop policy, rendering, generic section
  layout, focus reconciliation, connection presentation, and effect execution.
- A `System` owns its lanes, section definitions, snapshot collection, session
  control implementation, and divider-button semantics.
- `AppState` stores backend-neutral values (`LaneId`, `SectionDef`,
  `SessionSnapshot`, `LaneSnapshot`). It does not look up global backend
  objects.
- `SystemRegistry` is injected into `App` and `RefreshWorker`. It is the only
  lane-to-system router.

## Identity and data flow

`LaneId` is the stable key for one section. It encodes a system id and the
system's own lane id, so two systems can use the same lane name without a
collision.

```text
Config -> SystemRegistry::configure
       -> System::lanes / section_for -> AppState.system_sections -> layout
       -> System::snapshot            -> LaneRefresh              -> AppState
Action -> Effect::InvokeLaneAction    -> LaneActionProvider::invoke
                                      -> LaneShellIntent
```

The refresh worker asks the registry for snapshot routes. Foreground lanes are
applied immediately; background lanes are guarded by a single-flight gate and
sampled in parallel. Both paths return the same `LaneRefresh` type. Exclude
filters and result application are shell policies and therefore happen once,
outside concrete systems.

## Current trait contract

```rust
pub trait System: Send + Sync {
    fn id(&self) -> &str;
    fn configure(&self, config: &Config);
    fn lanes(&self) -> Vec<LaneId>;
    fn section_for(&self, lane: &LaneId) -> Option<SectionDef>;
    fn runtime(&self, lane: &LaneId) -> Option<LaneRuntime<'_>>;
}

pub trait LaneActionProvider: Send + Sync {
    fn invoke(
        &self,
        lane: &LaneId,
        action: &LaneActionId,
        anchor: LaneActionAnchor,
    ) -> Vec<LaneShellIntent>;
}
```

`LaneRuntime` composes optional catalog, session-control, lane-action,
configuration, attachment-lifecycle, focus-transport, and summary-transport
providers. A partial backend may expose only catalog and lane actions; the
registry fixture verifies that all other ports remain absent rather than being
filled with dummy implementations.

Divider buttons carry a typed `LaneActionId`. The shell echoes it to the
owning provider and executes only the returned generic `LaneShellIntent`; App
does not match system ids or backend action ids.

`SectionDef.primary` identifies the one lane backed by Deck's embedded local
terminal. Attachment, focus, summary, configuration, and lane-action behavior
are optional runtime capabilities; section presentation contains no connection
key.

## Extension test

`system::tests::partial_system_mounts_snapshot_and_actions_without_dummy_control`
mounts an independent fixture and verifies section discovery, snapshots, lane
actions, and the absence of unsupported configuration, attachment, focus, and
summary providers. This is the executable contract for the open/closed
boundary.

The persisted tmux/SSH config remains host-based for backward compatibility.
Only `TmuxSystem`, the SSH config adapter, attachment adapter, and remote
connection services translate that schema to or from `LaneId`.
