# Session abstraction — unify local & remote behind one backend

> Status: **planning only — do not implement yet.** Captured from a
> design discussion. Implementation will be done step by step under the
> user's direction.

## Goal

deck talks to one local tmux server and N remote ones over ssh. Today the
two paths are two parallel code bases (`tmux.rs` vs `remote_tmux.rs`,
`local_terminal` vs `remote_conns`, sync vs background-thread) that the
high-level layers keep branching on. This violates the rule now written
into `CLAUDE.md`:

> Local and remote produce the *same* shape … the high-level layers must
> not branch on local vs remote … push the local/remote split as low as
> it goes.

This doc defines a single `SessionBackend` trait, keyed by
`Option<String>` host (`None` = local, `Some(host)` = remote), so local
becomes a degenerate implementation of the same interface — not a special
case the rest of the app knows about.

## TL;DR

After auditing every operation on both sides (see the Venn below), only
**two** differences are essential (physical); everything else is
accidental and collapses into the shared trait:

1. **deck's process runs on the local machine** → "don't attach to the
   session deck itself lives in" (nesting) can only happen locally. It
   stays as a one-line App-level guard, not a trait method.
2. **Transport is ssh vs in-process** → hidden behind a `Transport` hint;
   the trait surface never mentions ssh.

Three design rules drive the collapse:

- **One data shape, keyed by `Option<String>` host.** No `foo` +
  `remote_foo` pairs.
- **Every session operation runs off the UI thread** — even local ones.
  This erases the sync/async asymmetry. Local just completes faster.
- **Don't assume any PTY is always present.** Local gets the same
  `status()` / respawn / empty-state machinery remote already has.

## The Venn diagram

```
        ┌──────────────────── shared (the trait body) ────────────────────┐
        │ spawn / respawn / status / is_alive / drain / write /           │
        │ resize / teardown / transport()                                  │
        │ list_sessions(Reachability) / current_session /                  │
        │ switch_to_session / rename / kill(name, switch_to) /             │
        │ new_session / persist_order / list_dir                           │
        └──────────────────────────────────────────────────────────────────┘
   local-only                                       remote-only
   └ enclosing_session() guard (one line)            └ (empty)
     — physical: deck runs locally                      transport is ssh,
                                                         but hidden behind
                                                         Transport, not in
                                                         the trait

   App layer (outside the Venn, cross-cutting):
   └ active: Option<String>  (which backend is on screen; None = local)
   └ the async executor (per-backend work queue + result channel)
   └ the one-line nesting guard, applied only on local switch
```

## What collapses, and how

### The single unlock: each Attachment knows its own client tty

Two of the apparent local-only primitives are really one problem.

- Local `switch-client -c <tty> -t <session>`
  (`switch_client_for_tty`, `src/infra/tmux.rs:191`) targets deck's *own*
  client by tty because the local server is shared with the user's other
  clients. Remote uses a bare `switch-client -t`
  (`remote_tmux::switch_client`, `src/infra/remote_tmux.rs:157`) and only
  gets away with it because deck's `ssh -tt host tmux attach` is assumed
  to be the only client on that server. That assumption breaks the moment
  the user opens their own tmux client on the remote — remote has the
  same latent "switch the wrong client" bug, it just bites less often.

- Highlighting the current session locally needs
  `current_session_for_tty` (`src/infra/tmux.rs:152`); remote tracks no
  current session at all (`apply_remote` has no current/ack field,
  `src/app/refresh.rs`).

Both unify once every backend holds **its own client tty**:

- `switch_to_session(name)` becomes one primitive; tty-targeting is an
  implementation detail, not a trait method. → `switch_to_tty` is deleted.
- `current_session()` becomes one primitive both sides implement.

**New work this requires:** local gets the client tty for free
(`master.tty_name()`, captured at `src/infra/pty.rs:38`). The remote
client tty lives on the remote end and isn't visible to the local `ssh`
process — it must be captured once after attach (e.g.
`ssh host tmux list-clients -F '#{client_tty}'`, picking deck's client).
First cut may keep remote's bare `switch-client` with a `TODO`; the trait
surface is already unified.

### Nesting guard — the one irreducible local-only thing

deck's process runs on the local machine, so a remote session can never
contain it: **you can't nest into a remote session.** This is physical,
not a fixable asymmetry. It is not a trait method.

What it actually protects: attaching deck's client to the session deck is
*itself* running inside → tmux renders the deck pane inside itself
(recursion / "size to smallest" shrink). Today this is a whole subsystem
(`NestingGuard`, `host_session()` at `src/infra/tmux.rs:146`,
`warning_for_switch`, `preferred_attach_target`).

Plan: **collapse to a one-line check** on the local switch path —
`if enclosing_session() == target { refuse/warn }`, where
`enclosing_session()` reads `$TMUX_PANE`'s session. Keep it out of the
trait, apply it only to local. Whether to drop it entirely is the user's
call: if deck is normally launched *outside* the sessions it manages, the
footgun almost never fires and the guard can go. Cut it with eyes open.

### Everything else (collapses cleanly)

| Today's asymmetry | Resolution |
|---|---|
| `switch_to_tty` local-only | Folds into `switch_to_session` (see unlock above). |
| `current_session_for_tty` local-only | `current_session()` on both; remote gains current tracking. |
| Proc-heuristic Claude status local-only (`list_panes` → `SessionStatus`, `src/infra/tmux.rs:105`) | Already superseded by the unified agent system (`agent::detect_agents`, `AppState.agents` keyed by `Option<String>`, per `CLAUDE.md`). Drop the heuristic = switch to the shared `agents` map, not a feature loss. |
| Remote kill ignores `switch_to` (`src/app/dispatch.rs:422`) | `kill(name, switch_to)` on both; both pre-switch the client off the doomed session before killing. |
| Local rename patches `session_order` in place (`src/app/dispatch.rs:400`) | Drop it; rely on next refresh + `@deck_order` re-sort like remote. Cost: ~1s order flicker after a rename. Acceptable. |
| Local death quits deck (`src/app/mod.rs:584`) | Show an empty "no sessions" state like remote. Also fixes deck vanishing out from under the user. |
| Local PTY assumed always present (`local_terminal`, no `Option`) | Local pane becomes nullable / respawnable, same machinery as remote. |
| `status()` / `connected()` remote-only (`RemoteConnStatus`, `src/app/mod.rs:58`) | Shared `ConnStatus` on both. Local is normally `Connected`, can be `Disconnected` (then respawn → empty state). With the async rule it may also pass briefly through `Connecting`. (Note: "local always returns true" contradicts "don't assume the PTY is always present" — local needs the real `Connected \| Disconnected`, not a constant.) |
| `active_remote` remote-only (`src/app/dispatch.rs:368`) | Already the multiplexer: `None` = local. Rename to `active: Option<String>`; local is the `None` participant, not an exception. Stays at App layer. |
| Tri-state reachability remote-only (`Option<Vec<…>>`, `src/infra/remote_tmux.rs:73`) | `Reachability<Vec<…>>` on both; local "no tmux server" → empty state instead of an infallible call. |
| ssh / ControlMaster / BatchMode / `REMOTE_PATH_PREFIX` (`src/infra/ssh.rs`, `src/infra/remote_tmux.rs:48`) | Physical transport detail. Hidden behind `Transport`; trait never mentions ssh. |
| `list_dir` over ssh (`src/infra/remote_tmux.rs:264`) | `list_dir(path)` on both; local uses `std::fs`, remote uses `ssh ls`. |
| Single-flight gating, auto-recovery, `pending_remote_switch`, onboard/offboard | Become uniform **executor** concerns (next section). |
| `@deck_order` persistence | **Already at parity** (`tmux::persist_session_order` + `remote_tmux::persist_session_order`). No work. |

## The biggest lever: all operations async → one executor

The rule "even local operations leave the UI thread" erases the entire
sync/async impedance. Once every session operation is *fire intent →
worker runs it → result applied on the UI thread*, local is just the fast
case. deck already has this pattern — the **refresh worker**
(`src/infra/refresh.rs`). The design generalizes it from "list only" to
"all session operations."

Things that are remote-only today become uniform properties of this
executor:

- **single-flight / in-flight gating** (`remote_in_flight`,
  per-host `Connecting` guard) → the executor's per-backend serialization.
- **`pending_remote_switch`** (`src/app/dispatch.rs:879`) → a queued item.
- **auto-recovery respawn** (`hosts_needing_respawn`,
  `src/app/refresh.rs:229`) → one recovery policy, local PTY death runs
  the same path.

**The one real risk here.** Going async for local means the executor must
guarantee **per-backend ordering / causality**: a `switch_to_session`
followed by a `list_sessions` must not let a stale list overwrite the new
state. The remote `remote_in_flight` single-flight exists for exactly
this; generalizing it to local must be designed explicitly. This is the
only place the sync→async move can bite.

## Proposed trait

```rust
enum ConnStatus { Connecting, Connected, Disconnected } // local rarely Connecting; can Disconnected
enum Transport  { InProcess, Ssh }                       // executor schedules by this; trait body ignores it

trait SessionBackend {                 // one impl for local, one for remote
    fn transport(&self) -> Transport;
    fn status(&self) -> ConnStatus;
    fn is_alive(&self) -> bool;

    // attachment / transport lifecycle
    fn spawn(&mut self, size: PtySize);
    fn respawn(&mut self);
    fn drain(&mut self) -> Vec<PtyEvent>;
    fn write(&mut self, bytes: &[u8]);
    fn resize(&mut self, size: PtySize);
    fn teardown(&mut self);

    // session control plane (all run via the async executor)
    fn list_sessions(&self) -> Reachability<Vec<SessionInfo>>;
    fn current_session(&self) -> Option<String>;        // via this backend's own client tty
    fn switch_to_session(&self, name: &str);             // tty-targeting is impl-private
    fn rename(&self, old: &str, new: &str);
    fn kill(&self, name: &str, switch_to: Option<&str>);
    fn new_session(&self, name: &str, dir: &str) -> bool;
    fn persist_order(&self, order: &[String]);
    fn list_dir(&self, path: &str) -> (Vec<String>, Option<String>);
}

// backends: HashMap<Option<String>, Box<dyn SessionBackend>>
//   None = local, seeded at startup, never offboarded.
//   Some(host) = remote, onboarded/offboarded from config.
```

Cross-cutting, stays at the App layer (outside the trait):

- `active: Option<String>` — which backend drives the main pane.
- the async executor — per-backend queue + result channel + ordering.
- the one-line nesting guard — applied only when `active`/target is local.

## Risk & work checklist

Most of the change is *deleting* branches. Only these are genuinely new
work or real trade-offs:

1. **Remote client-tty capture** — the only new mechanism; unlocks the
   `switch_to_session` / `current_session` unification.
2. **Async executor per-backend ordering** — the one place sync→async can
   introduce stale-state races. Design the serialization explicitly.
3. **Local no longer quits; shows an empty state** — behaviour change,
   needs the empty-state UI to cover the local case.
4. **Nesting guard: shrink to one line vs cut entirely** — user's call;
   cutting accepts the attach-into-own-session footgun.
5. **Drop the proc-status heuristic** — confirm it routes to the existing
   unified `agents` system, not a plain feature removal.

## Implementation order (proposed, to confirm before coding)

1. Land the trait + `Transport` / `ConnStatus` / `Reachability` types with
   the **local** backend implemented sync-inline first (no behaviour
   change), to prove the surface fits today's local code.
2. Move the **remote** backend behind the same trait (mostly re-homing
   `remote_tmux.rs`).
3. Introduce the **async executor**, route both backends through it, add
   per-backend ordering. (Biggest risk step — do it once both backends
   share the trait.)
4. Make local nullable/respawnable + empty state (kill the quit path).
5. Capture remote client-tty; unify `switch_to_session` /
   `current_session` fully; delete `switch_to_tty`.
6. Collapse nesting to one line (or cut); drop the proc heuristic; drop the
   local rename order-patch; unify `kill(switch_to)`.
