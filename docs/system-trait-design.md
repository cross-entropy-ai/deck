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

Divider buttons carry a typed `LaneActionId`. The shell echoes it to the
owning provider and executes only the returned generic `LaneShellIntent`; App
does not match system ids or backend action ids.

`SectionDef.primary` identifies the one lane backed by Deck's embedded local
terminal; absence of a runtime key alone does not imply that role.
`SectionDef.runtime_key` is an optional opaque connection key. Generic routing
still uses `LaneId`; the key exists only for shell-owned attachment workflows
that need to correlate a section with a persistent client. tmux uses the SSH
host name.

## Extension test

`system::tests::second_system_mounts_sections_snapshots_and_control_without_shell_changes`
mounts an independent fixture system and verifies section discovery, snapshot
routing, and control dispatch. This is the executable contract for the open/
closed boundary.

## Intentional compatibility seam

PTY attachment and remote connection lifecycle predate `System` and remain
shell services. Consequently `SessionEntry.host` is retained as a presentation/
attachment compatibility value for now. Backend ownership, refresh routing,
control routing, model keys, and layout no longer depend on it. Removing that
last seam requires generalizing terminal attachment itself, not another session
DTO migration.
