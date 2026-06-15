# System trait: deck as a general framework

Status: in progress. Turns deck from a tmux-specific TUI into a **shell** that
mounts **Systems**. tmux (local + remote) becomes one built-in `System`; adding
a non-tmux backend = `impl System` + register, with no shell changes.

## Responsibility boundary

- **Shell (kernel)** owns: terminal panes, event loop, render, layout skeleton,
  theme, the sectioned-list widget, hit-testing. It does **not** know tmux/ssh
  or "local vs remote".
- **System** owns: which lanes exist, each lane's divider title/buttons/badge,
  how to snapshot sessions+agents, the control plane (switch/kill/rename/…),
  and what a divider button does.

## LaneId (replaces HostKey)

The old key was `HostKey` = `Option<String>` (None=local, Some=host), tmux-only.
With multiple systems that's ambiguous, so the key carries **system identity**:

`LaneId(Arc<str>)` encoded as `"{system}\x1f{lane}"` (unit separator, never in a
system id or host name). `system()` / `lane()` split it back; `Borrow<str>` keeps
map lookups allocation-free when you hold a `LaneId`. The shell's old local-lane
`None` becomes `LaneId::new("tmux", "local")`; a remote host becomes
`LaneId::new("tmux", host)`. The shell no longer has a local/remote concept;
`@local` / `@host` titles come from `SectionDef`.

## The trait

```rust
pub trait System {
    fn id(&self) -> &str;
    fn sections(&self, ctx: &SystemCtx) -> Vec<SectionDef>;   // structure
    fn snapshot(&self, lane: &LaneId) -> Option<LaneSnapshot>; // discovery (refresh)
    fn control(&self, lane: &LaneId) -> Box<dyn SessionControl + Send>;
    fn on_button(&self, lane: &LaneId, command: &str) -> Vec<Effect>; // interaction
}
```

Types: `SectionDef { lane, title, accent, buttons: Vec<SectionButton>, badge,
top_margin }`, `SectionButton { glyph, command }`, `Badge { label, status }`,
`LaneSnapshot { sessions: Vec<SessionInfo>, agents: Vec<DetectedAgent> }`,
`SystemCtx<'a>` (read-only shell state a system needs to build sections, e.g.
config + forward health). `SessionControl` is unchanged.

## Button routing (decision A)

One generic action variant `Action::SystemButton { lane, command }`; the reducer
arm delegates straight to `system.on_button(...)` and runs the returned effects.
This replaces the closed `DividerButton` enum + its per-button arms, so the
reducer stops growing per feature.

## Plan (each step compiles + tests)

1. ✅ `LaneId` type (model/lane.rs).
2. ✅ `System` trait + types (src/system/).
3. ✅ `TmuxSystem` wrapping local+remote.
4. ✅ Re-key in-memory stores (agents, collapsed_sections, executor lanes) to `LaneId`; `HostKey` deleted.
5. ◑ Divider DTOs (`SectionMeta`/`DividerHit`) → `LaneId`. **Deferred:** routing DTOs (`Kill`/`Rename`/`Create`), `SessionEntry.host`, `AgentTarget` still carry `Option<String>` (bridged via `system::tmux::lane`).
6. ✅ Sidebar built from `system.section_for(lane)`; `DividerButton`/`local_divider`/`remote_divider` removed; `ForwardBadge` → `system::Badge`.
7. ✅ Refresh gathers per-lane via `System::snapshot` (orchestration — current_session, exclude, status, order, reachability — stays in the worker; `RefreshUpdate`/apply unchanged).
8. ✅ Decision A: mouse → `Action::SystemButton` → `System::on_button` → effects.
9. ◑ Control plane via `System::control`; `for_lane` registry (multi-system routing). **Deferred:** drop `SessionOrigin`; finish the routing/entry DTO migration.

The `System` trait is fully wired (`section_for`, `snapshot`, `control`,
`on_button`, resolved via `for_lane`). Mounting a second backend = `impl
System` + add it to `SYSTEMS`; the remaining `Option<String>` DTOs would need
finishing for a non-tmux backend to round-trip cleanly.
