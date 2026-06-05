# High-ROI cleanup plan

## Goal

Reduce the highest-friction redundancy without changing deck's user-visible
behavior. The current best target is the reducer/dispatcher effect boundary:
`SideEffect` is a field-heavy command bag whose merge logic must be updated
every time a new effect is added.

## Phase 1: Replace the SideEffect field bag

Problem:

- `SideEffect` stores one field per command.
- `SideEffect::merge` repeats every field manually.
- Dispatch has to know the whole field layout.

Plan:

- Introduce an ordered `Effect` enum.
- Make `SideEffect` a small wrapper around `Vec<Effect>`.
- Add helper methods so reducers append effects directly.
- Dispatch by iterating over `Effect` values.

Expected result:

- New reducer effects need one enum variant and one dispatch arm.
- Compound actions compose by appending effects, not copying fields.
- Effect ordering is explicit.

## Phase 2: Collapse new-session picker construction

Problem:

- Local and remote new-session picker setup share most fields.
- Validation branches repeat the same name rules with different session
  sources.

Plan:

- Introduce a `NewSessionTarget` helper: host, start directory, existing names.
- Build `NewSessionState` through one constructor path.
- Keep target-specific directory validation at the final boundary.

Expected result:

- Less duplication in `app/dispatch.rs`.
- Easier future changes to picker behavior.

## Phase 3: Finish or shrink SessionControl

Problem:

- `SessionControl` has a good direction, but part of the trait is still
  staged behind dead-code allowances.
- Mutating ops use the executor, while refresh/list/current still use the
  older infra paths.

Plan:

- Either route refresh/list/current through `SessionControl`, or remove those
  methods until they are needed.
- Keep PTY lifecycle outside the trait until there is a concrete migration.

Expected result:

- Trait methods are load-bearing.
- The local/remote boundary stays lower in the stack.

## Phase 4: Extract orchestration managers

Problem:

- `App` owns too many independent orchestration concerns: PTYs, remote
  connection lifecycle, plugins, session commands, refresh, update, and port
  forwarding.

Plan:

- Extract manager structs one at a time.
- Start with remote connection lifecycle because it is the most stateful.
- Keep `App` as the event-loop coordinator.

Expected result:

- Smaller `dispatch.rs` and `mod.rs`.
- Fewer unrelated concepts changing in the same file.
