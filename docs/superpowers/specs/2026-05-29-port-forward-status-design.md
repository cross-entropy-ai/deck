# Port Forward Status Display + Liveness Probe — Design

Date: 2026-05-29
Status: Draft

Follow-up to `2026-05-28-port-forward-design.md` (that feature shipped). This
adds an at-a-glance liveness indicator for the forwards it manages.

## Summary

Surface per-host port-forward liveness in the sidebar. Each remote host divider
gains a `⇄N` badge (hidden when the host has no forwards) colored by a per-host
rollup of forward health. A background probe enumerates the machine's local
listening TCP ports once per refresh tick (1s) to confirm `-L` / `-D`
listeners; `-R` forwards can't be seen locally, so they degrade to "presumed
from master state". The port-forward overlay gains a per-forward health dot so
the user can tell *which* forward is down.

The status is shown **per remote section** (on each host's divider), not as a
single aggregate. Port forwards are intrinsically per-host, the divider already
carries per-host connection status (the `[⟳]` glyph, tinted by `HostStatus`),
and a per-host badge gives "which host is broken" for free instead of forcing a
drill-down.

## Goals

- Per-host forward health visible in the sidebar without opening the overlay.
- **Non-intrusive** probing: never open a connection *through* a tunnel (no
  traffic reaches the remote target as a side effect of probing).
- Per-forward up/down detail inside the overlay.
- Reuse the existing worker thread (`app/port_forward_task.rs`), the existing
  1s refresh tick, and the existing divider renderer.

## Non-goals

- End-to-end probing (actually reaching the remote target service). Explicitly
  rejected: it would touch the user's services and be protocol-specific.
- Remote-side probing for `-R` (would require an `ssh host ss -ltn` round-trip
  every cycle). `-R` degrades to presumed-from-master-state.
- A clickable badge. It is status-only; `[…]` and the `f` key already open the
  overlay.
- Probing when a host has no forwards, or when no remotes are configured.
- A `N/M` (up-of-total) badge. MVP shows count + color; `N/M` is deferred.

## Architecture

```
src/
  infra/
    listeners.rs           NEW — pure parse of `netstat`/`ss` output into a
                                 HashSet<u16> of local LISTEN ports, + the
                                 platform-appropriate Command builder.
  app/
    port_forward_task.rs   MODIFY — Op::Probe; worker enumerates listeners once
                                    and returns per-forward health.
    refresh.rs / mod.rs    MODIFY — each REFRESH_INTERVAL tick, dispatch Probe
                                    with the host/forward set from config_remotes
                                    (only when ≥1 forward exists).
    dispatch.rs            MODIFY — reducer handles the probe result, writes
                                    AppState.forward_health; derives -R health
                                    from master state.
  model/
    state.rs               MODIFY — ForwardHealth, ForwardKey, forward_health
                                    map, host_pf_badge() rollup; SidebarItemData
                                    ::Header gains a pf badge field.
  ui/
    sidebar.rs             MODIFY — render_group_header emits `⇄N`, reserves its
                                    width from the rule run.
    overlays/port_forward.rs MODIFY — draw_list prefixes each row with a health
                                    dot.

tests/unit/
  infra/listeners.rs       NEW — parse macOS + Linux samples → port sets.
  model/state.rs           MODIFY — rollup color, -R/-L/-D health derivation,
                                    ForwardKey prune on reload.
  app/port_forward_task.rs MODIFY — Op::Probe via Runner stub.
```

### Layering rationale

- `infra/listeners.rs` is pure parsing + a `Command` builder, no threads — same
  role as `infra/port_forward.rs`. The probe's only I/O (spawning `netstat`/`ss`)
  happens on the worker thread that already owns all forward I/O.
- The worker computes probe health from `(listening-port set, masters_up)` — it
  already tracks `masters_up`, so it can answer `-R` too — and returns one result
  per forward. The reducer just stores them in `forward_health`. The probe is
  authoritative each tick; there is no stateful merge.

## Data model

### Health

```rust
// model/state.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForwardHealth {
    Probing,   // not yet probed this session (transient, first tick)
    Up,        // -L/-D: local listener present
    Down,      // -L/-D: no local listener; or any mode whose last apply failed
    Presumed,  // -R: master is up, cannot be locally confirmed
}
```

### Key

A forward's identity, stable across config reloads and reorders. A local listen
port is unique per machine (ssh fails to bind a second forward on the same
port), so `(host, mode, bind_addr, listen_port)` uniquely names a forward.

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ForwardKey {
    pub host: String,
    pub mode: ForwardMode,
    pub bind_addr: Option<String>,
    pub listen_port: u16,
}
```

(`ForwardMode` and `ForwardSpec` already derive the needed traits; add `Hash`
where required.)

### Storage

```rust
// AppState
pub forward_health: std::collections::HashMap<ForwardKey, ForwardHealth>,
```

Pruned on config reload: drop entries whose key no longer appears in
`config_remotes`, so removed forwards don't linger as stale dots.

### Rollup → badge

```rust
pub struct PfBadge { pub count: usize, pub color: PfBadgeColor }
pub enum PfBadgeColor { Healthy, Degraded, Probing } // → green / pink / yellow
```

`host_pf_badge(host) -> Option<PfBadge>`:

- no forwards for host → `None` (badge hidden).
- `count` = number of forwards on the host.
- color: **any `Down` → Degraded (pink)**; else **any `Probing` → Probing
  (yellow)**; else **Healthy (green)**. `Presumed` and `Up` both read as green
  when nothing is worse.

## Probe

### What the worker does

New op:

```rust
// app/port_forward_task.rs
pub enum Op {
    // ... existing ...
    Probe { items: Vec<ProbeItem> },
}
pub struct ProbeItem { pub key: ForwardKey, pub mode: ForwardMode }
```

On `Probe`:

1. If any item is `-L`/`-D`, enumerate local LISTEN ports **once**:
   `infra::listeners::local_listen_ports()` → `HashSet<u16>`.
2. Per item:
   - `Local`/`Dynamic`: `Up` if `listen_port ∈ set`, else `Down`.
   - `Remote`: `Presumed` if `masters_up.contains(host)`, else `Down`.
3. Return one result per item: `ProbeResult { key, health }` (a new variant on
   the existing worker→UI result channel).

`netstat`/`ss` is spawned at most once per tick, and only when at least one
`-L`/`-D` forward exists.

### Listener enumeration (`infra/listeners.rs`)

| Platform | Command                 | Local-addr column example | Port extraction        |
| -------- | ----------------------- | ------------------------- | ---------------------- |
| macOS    | `netstat -an -p tcp`    | `127.0.0.1.8080` / `*.8080` / `::1.8080` | rsplit on `.` |
| Linux    | `ss -ltn`               | `0.0.0.0:8080` / `[::]:8080` / `*:8080`  | rsplit on `:` |

Keep only rows in `LISTEN` state. Pure fn `parse_listen_ports(os, output) ->
HashSet<u16>` so the parser is unit-tested against captured sample output for
both formats (incl. IPv6 and wildcard binds, multiple ports).

We key on port alone, ignoring `bind_addr` — the question is only "is the
listener up", and the listen port is unique per machine regardless of bind.

### Cadence

Dispatched from the existing main-loop tick (`REFRESH_INTERVAL = 1s`,
`app/mod.rs:33`), alongside the remote-session refresh. Skipped entirely when no
forwards are configured.

### `-R`

`-R` listens on the remote host, invisible to local enumeration, so it shows
`Presumed` whenever the master is up and `Down` when the master is down. A
failed `-R` apply is never persisted to config (the prior design's AddForward
path only persists on success), so a failed forward is never probed. deck cannot
detect a remote-side bind that drops *after* a successful apply — that is
inherent to local-only probing, and is surfaced honestly as `Presumed` (the `○`
dot) rather than a confirmed green `Up`.

## UI

### Sidebar badge (per host divider)

`render_group_header` (`sidebar.rs:384`) currently lays out
`leading + label + spacer + rule + [⟳] + […]`. Add a `⇄N` badge between the rule
and `[⟳]`, present only when `host_pf_badge(host)` is `Some`:

```
@server-1 ─────────────── ⇄2 [⟳] […]     ⇄2 green = both Up
@server-2 ───────────────────── [⟳] […]   no forwards → no badge
@server-3 ─────────────── ⇄3 [⟳] […]     ⇄3 pink  = one of 3 is Down
@db ───────────────────── ⇄1 [⟳] […]     ⇄1 green = -R only (Presumed; see overlay ○)
```

- Glyph `⇄` (U+21C4, single cell), then the decimal count.
- Width = `1 + digits + gap`, subtracted from `rule_w` the same way the two
  buttons already are (so total divider width stays constant).
- Color from `PfBadge.color`: green = `theme.green`, pink = `theme.pink`,
  yellow = `theme.yellow`.
- The badge is **not** a hit target; only `[⟳]` and `[…]` register `DividerHit`s.

`render_group_header` gains a `pf: Option<PfBadge>` parameter; the `Header` match
arm in the sessions renderer computes it via `host_pf_badge`. (Carry the badge
on `SidebarItemData::Header` next to the existing `status` field, computed when
the layout is built.)

### Overlay per-forward dot

`draw_list` (`overlays/port_forward.rs`) prefixes each forward row with a health
dot, looked up from `forward_health` by `ForwardKey`:

```
┌─ Port Forward — server-3 ─────────────────────────────┐
│                                                       │
│  ● > -L localhost:8080  → localhost:80                │   ● green = Up
│  ✕   -L 127.0.0.1:5433  → db:5432                     │   ✕ pink  = Down
│  ○   -R 0.0.0.0:9090    → localhost:5432              │   ○ dim   = Presumed (-R)
│  · > -D 1080                                          │   · muted = Probing
│                                                       │
│  [a] add   [d] delete   [esc] close                   │
└───────────────────────────────────────────────────────┘
```

`draw_list` takes the host's health (the `forward_health` map + host, or a
pre-built slice aligned to `forwards`) so each row resolves its own dot.

## Error handling

| Scenario                                   | Behavior                                                                 |
| ------------------------------------------ | ------------------------------------------------------------------------ |
| `netstat`/`ss` missing or non-zero exit    | `-L`/`-D` health stays `Probing` (not flipped to `Down` — avoid false alarms); badge shows yellow; warning logged once. |
| Probe spawn fails                          | Same as above.                                                           |
| Unsupported platform (not macOS/Linux)     | No enumeration; badge shows the count in a neutral color and makes no liveness claim (documented limitation). |
| Forward removed via reload                 | Its `ForwardKey` entry is pruned from `forward_health`; no stale dot.    |
| Non-ssh process holding the listen port    | Reads as `Up` (false positive). Accepted: low likelihood, noted in Risks. |

## Testing

Pure unit tests:

- `parse_listen_ports(os, output)`: macOS `netstat -an -p tcp` sample and Linux
  `ss -ltn` sample → expected `HashSet<u16>`. Cover IPv4, IPv6, wildcard binds,
  multiple listeners, and non-LISTEN rows that must be excluded.
- `host_pf_badge`: vectors of `ForwardHealth` → expected `PfBadgeColor` for the
  all-Up, contains-Down, contains-Probing, and Presumed-only cases; empty → `None`.
- Health derivation: given a listening-port set + master-up flag, assert `-L`/`-D`
  Up/Down and `-R` Presumed/Down.
- `ForwardKey` prune: after a reload that drops a forward, its health entry is gone.

Worker:

- `Op::Probe` via the `Runner`/enumeration stub: assert one `ProbeResult` per
  item with correct health, and that enumeration runs at most once per `Probe`.

Manual:

- Configure `-L 8080:localhost:80` to a reachable host → badge green, overlay dot ●.
- `kill` the master externally → within ~1s badge flips pink, dot ✕.
- Configure a `-R` forward → badge/​dot show Presumed (○) while master is up.
- A host with no forwards shows no badge.

## Open questions

1. **Badge glyph.** `⇄` (U+21C4) vs `⇅` vs `⊶`. Defaulting to `⇄`; it has wide
   terminal-font coverage. If a fallback is needed, ASCII `<>`.
2. **Count vs ratio.** MVP renders `⇄N` + color. `⇄N/M` (up of total) is
   deferred unless the color rollup proves too coarse in practice.

## Risks

- **Port-reuse false positive.** If a forward dies and a non-ssh process grabs
  the same local port, the probe reads `Up`. Low likelihood (ssh held the port);
  accepted for MVP.
- **1s `netstat`/`ss` spawn.** One short-lived process per tick, only when
  forwards exist. Negligible cost; matches the existing per-tick remote refresh.
- **`-R` blind spot.** `-R` liveness is never truly confirmed (only presumed
  from master state). This is inherent to local-only probing and is surfaced
  honestly via the distinct `Presumed` dot rather than a green `Up`.
