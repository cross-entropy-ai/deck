# Port Forward Status Display + Liveness Probe — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Show each remote host's port-forward liveness as a colored `⇄N` badge on its sidebar divider, backed by a non-intrusive probe that enumerates local listening ports once per refresh tick; show per-forward up/down dots in the overlay.

**Architecture:** A pure listener-enumeration helper (`infra/listeners.rs`) feeds the existing port-forward worker thread, which gains a `Probe` op that classifies each forward into a `ForwardHealth`. Results flow back through the existing worker→reducer channel into a `forward_health` map on `AppState`. The sidebar divider renderer and the port-forward overlay read that map. `-L`/`-D` are confirmed by local listener presence; `-R` degrades to "presumed from master state".

**Tech Stack:** Rust, ratatui (rendering), crossterm, std `mpsc` channels + threads, `unicode-width` (already used in `sidebar.rs`).

**Design doc:** `docs/superpowers/specs/2026-05-29-port-forward-status-design.md`

---

## Task 1: Local listener enumeration (`infra/listeners.rs`)

Pure parsers for `netstat`/`ss` output, plus a platform-dispatching
`local_listen_ports()`. `None` means "couldn't enumerate" (unsupported OS or
command failure) so callers can tell that apart from "checked, port absent".

**Files:**
- Create: `src/infra/listeners.rs`
- Create: `tests/unit/infra/listeners.rs`
- Modify: `src/infra/mod.rs` (add `pub mod listeners;`)

- [ ] **Step 1: Write the failing tests**

Create `tests/unit/infra/listeners.rs`:

```rust
use crate::infra::listeners::{parse_netstat, parse_ss};

#[test]
fn netstat_extracts_listen_ports() {
    let sample = "\
Proto Recv-Q Send-Q  Local Address          Foreign Address        (state)
tcp4       0      0  127.0.0.1.8080         *.*                    LISTEN
tcp6       0      0  ::1.8080               *.*                    LISTEN
tcp46      0      0  *.1080                 *.*                    LISTEN
tcp4       0      0  127.0.0.1.52345        93.184.216.34.443      ESTABLISHED";
    let ports = parse_netstat(sample);
    assert!(ports.contains(&8080), "8080 should be LISTEN");
    assert!(ports.contains(&1080), "1080 should be LISTEN");
    assert!(!ports.contains(&52345), "ESTABLISHED row must be ignored");
    assert_eq!(ports.len(), 2, "8080 appears twice (v4+v6), deduped, plus 1080");
}

#[test]
fn ss_extracts_listen_ports() {
    let sample = "\
State      Recv-Q Send-Q Local Address:Port  Peer Address:Port
LISTEN     0      128    0.0.0.0:8080        0.0.0.0:*
LISTEN     0      128    [::]:8080           [::]:*
LISTEN     0      4096   127.0.0.1:1080      0.0.0.0:*
LISTEN     0      128    *:9090              *:*";
    let ports = parse_ss(sample);
    assert!(ports.contains(&8080));
    assert!(ports.contains(&1080));
    assert!(ports.contains(&9090));
    assert_eq!(ports.len(), 3);
}

#[test]
fn ignores_header_and_empty() {
    assert!(parse_ss("State Recv-Q Send-Q Local Address:Port Peer Address:Port").is_empty());
    assert!(parse_netstat("Active Internet connections (including servers)").is_empty());
    assert!(parse_ss("").is_empty());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib listeners`
Expected: FAIL — `unresolved import crate::infra::listeners`.

- [ ] **Step 3: Create the module**

Create `src/infra/listeners.rs`:

```rust
//! Local TCP listener enumeration for port-forward liveness.
//!
//! Pure parsers for `netstat` (macOS) and `ss` (Linux) output, plus a
//! platform-dispatching `local_listen_ports()` that shells out and parses.
//! Returns `None` when enumeration is unavailable (unsupported OS or the
//! command failed) so callers can distinguish "couldn't check" from
//! "checked, port absent".

use std::collections::HashSet;

/// Parse macOS `netstat -an -p tcp` output into the set of local ports in
/// LISTEN state. Local address is the 4th column; the port is the text after
/// the final `.` (e.g. `127.0.0.1.8080`, `*.1080`, `::1.8080`).
pub fn parse_netstat(output: &str) -> HashSet<u16> {
    let mut ports = HashSet::new();
    for line in output.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.last() != Some(&"LISTEN") {
            continue;
        }
        let Some(local) = cols.get(3) else { continue };
        if let Some((_, port)) = local.rsplit_once('.') {
            if let Ok(p) = port.parse::<u16>() {
                ports.insert(p);
            }
        }
    }
    ports
}

/// Parse Linux `ss -ltn` output into the set of local ports in LISTEN state.
/// Rows start with `LISTEN`; local address is the 4th column; the port is the
/// text after the final `:` (e.g. `0.0.0.0:8080`, `[::]:8080`, `*:9090`).
pub fn parse_ss(output: &str) -> HashSet<u16> {
    let mut ports = HashSet::new();
    for line in output.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.first() != Some(&"LISTEN") {
            continue;
        }
        let Some(local) = cols.get(3) else { continue };
        if let Some((_, port)) = local.rsplit_once(':') {
            if let Ok(p) = port.parse::<u16>() {
                ports.insert(p);
            }
        }
    }
    ports
}

/// Enumerate local TCP ports in LISTEN state. `None` means enumeration was not
/// possible (unsupported OS or the command failed) — callers should treat that
/// as "unknown", not "nothing listening".
pub fn local_listen_ports() -> Option<HashSet<u16>> {
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("netstat")
            .args(["-an", "-p", "tcp"])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        Some(parse_netstat(&String::from_utf8_lossy(&out.stdout)))
    }
    #[cfg(target_os = "linux")]
    {
        let out = std::process::Command::new("ss").args(["-ltn"]).output().ok()?;
        if !out.status.success() {
            return None;
        }
        Some(parse_ss(&String::from_utf8_lossy(&out.stdout)))
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        None
    }
}

#[cfg(test)]
#[path = "../../tests/unit/infra/listeners.rs"]
mod tests;
```

- [ ] **Step 4: Register the module**

In `src/infra/mod.rs`, add the declaration in alphabetical position (after `pub mod instance_guard;` / before `pub mod nesting_guard;` — match the file's ordering):

```rust
pub mod listeners;
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --lib listeners`
Expected: PASS (3 tests).

- [ ] **Step 6: Commit**

```bash
git add src/infra/listeners.rs src/infra/mod.rs tests/unit/infra/listeners.rs
git commit -m "feat(pf): pure local listener enumeration helper"
```

---

## Task 2: Health/key/badge types + rollup (`config.rs`, `state.rs`)

`ForwardHealth`, `ForwardKey` (stable identity across reloads), `PfBadge`/
`PfBadgeColor`, and the pure `rollup_color`. Add `Hash` to `ForwardMode` so
`ForwardKey` can derive `Hash`.

**Files:**
- Modify: `src/model/config.rs:48` (add `Hash` to `ForwardMode` derive)
- Modify: `src/model/state.rs` (new types + `rollup_color`, after the `HostStatus` enum near line 209)
- Modify: `tests/unit/model/state.rs` (append rollup tests)

- [ ] **Step 1: Write the failing tests**

Append to `tests/unit/model/state.rs`:

```rust
#[test]
fn rollup_down_dominates() {
    use crate::state::{rollup_color, ForwardHealth, PfBadgeColor};
    let healths = [ForwardHealth::Up, ForwardHealth::Down, ForwardHealth::Probing];
    assert_eq!(rollup_color(&healths), PfBadgeColor::Degraded);
}

#[test]
fn rollup_probing_when_no_down() {
    use crate::state::{rollup_color, ForwardHealth, PfBadgeColor};
    let healths = [ForwardHealth::Up, ForwardHealth::Probing];
    assert_eq!(rollup_color(&healths), PfBadgeColor::Probing);
}

#[test]
fn rollup_healthy_when_up_and_presumed() {
    use crate::state::{rollup_color, ForwardHealth, PfBadgeColor};
    let healths = [ForwardHealth::Up, ForwardHealth::Presumed];
    assert_eq!(rollup_color(&healths), PfBadgeColor::Healthy);
}

#[test]
fn forward_key_from_spec_uses_mode_bind_and_listen() {
    use crate::config::{ForwardMode, ForwardSpec};
    use crate::state::ForwardKey;
    let spec = ForwardSpec {
        mode: ForwardMode::Local,
        bind_addr: Some("127.0.0.1".into()),
        listen_port: 8080,
        target_host: Some("h".into()),
        target_port: Some(80),
    };
    let key = ForwardKey::from_spec("server-1", &spec);
    assert_eq!(key.host, "server-1");
    assert_eq!(key.mode, ForwardMode::Local);
    assert_eq!(key.bind_addr.as_deref(), Some("127.0.0.1"));
    assert_eq!(key.listen_port, 8080);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib rollup`
Expected: FAIL — `rollup_color`, `ForwardHealth`, `ForwardKey` not found.

- [ ] **Step 3: Add `Hash` to `ForwardMode`**

In `src/model/config.rs:48`, change:

```rust
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ForwardMode {
```

to:

```rust
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum ForwardMode {
```

- [ ] **Step 4: Add the types + rollup**

In `src/model/state.rs`, immediately after the `HostStatus` enum (ends at line 209), insert:

```rust
/// Liveness of a single configured forward, refreshed each probe tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForwardHealth {
    /// Not yet probed this session, or enumeration was unavailable.
    Probing,
    /// `-L`/`-D`: a local listener is present on the listen port.
    Up,
    /// `-L`/`-D`: no local listener; or any mode whose master is down.
    Down,
    /// `-R`: master is up, but the remote-side listener cannot be confirmed
    /// locally.
    Presumed,
}

/// Stable identity of a configured forward, used to key liveness across config
/// reloads and reorders. A local listen port is unique per host, but `mode` and
/// `bind_addr` are included so an `-L` and an `-R` sharing a port number (one
/// local, one remote) don't collide.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ForwardKey {
    pub host: String,
    pub mode: crate::config::ForwardMode,
    pub bind_addr: Option<String>,
    pub listen_port: u16,
}

impl ForwardKey {
    pub fn from_spec(host: &str, spec: &crate::config::ForwardSpec) -> Self {
        Self {
            host: host.to_string(),
            mode: spec.mode,
            bind_addr: spec.bind_addr.clone(),
            listen_port: spec.listen_port,
        }
    }
}

/// Per-host port-forward badge shown on the sidebar divider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PfBadge {
    pub count: usize,
    pub color: PfBadgeColor,
}

/// Rolled-up health color for a host's forwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PfBadgeColor {
    /// All forwards Up or Presumed → green.
    Healthy,
    /// At least one Down → pink.
    Degraded,
    /// At least one Probing, none Down → yellow.
    Probing,
}

/// Roll a host's per-forward healths into one badge color. `Down` dominates,
/// then `Probing`, else `Healthy` (`Up`/`Presumed`).
pub fn rollup_color(healths: &[ForwardHealth]) -> PfBadgeColor {
    if healths.iter().any(|h| *h == ForwardHealth::Down) {
        PfBadgeColor::Degraded
    } else if healths.iter().any(|h| *h == ForwardHealth::Probing) {
        PfBadgeColor::Probing
    } else {
        PfBadgeColor::Healthy
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --lib rollup && cargo test --lib forward_key`
Expected: PASS (4 tests).

- [ ] **Step 6: Commit**

```bash
git add src/model/config.rs src/model/state.rs tests/unit/model/state.rs
git commit -m "feat(pf): ForwardHealth, ForwardKey, badge rollup types"
```

---

## Task 3: `forward_health` map + `host_pf_badge` + reload prune (`state.rs`, `dispatch.rs`)

Store probe results on `AppState`, expose a per-host badge, and prune stale keys
on config reload.

**Files:**
- Modify: `src/model/state.rs` (struct field at ~605, init at ~745, new methods)
- Modify: `src/app/dispatch.rs:633` (prune after reload)

- [ ] **Step 1: Add the `use` for `HashMap` (if absent)**

At the top of `src/model/state.rs`, ensure this import exists (add it if it is
not already present):

```rust
use std::collections::HashMap;
```

Run `grep -n "use std::collections" src/model/state.rs` first; only add if
`HashMap` isn't already imported.

- [ ] **Step 2: Add the struct field**

In `src/model/state.rs`, in `pub struct AppState`, after the `config_remotes`
field (line 669):

```rust
    /// Per-forward liveness, refreshed each probe tick by the port-forward
    /// worker. Keyed by `ForwardKey`. Missing key = `Probing` (not yet seen).
    pub forward_health: HashMap<ForwardKey, ForwardHealth>,
```

- [ ] **Step 3: Initialize it in the constructor**

In `AppState::new`, after `config_remotes: Vec::new(),` (line 745):

```rust
            forward_health: HashMap::new(),
```

- [ ] **Step 4: Add `host_pf_badge` and `prune_forward_health` methods**

In `src/model/state.rs`, inside `impl AppState` (anywhere after `sidebar_layout`,
e.g. right after it ends at line 969):

```rust
    /// The port-forward badge for a host's divider, or `None` when the host has
    /// no forwards. Color rolls up the per-forward health; count is the number
    /// of configured forwards.
    pub fn host_pf_badge(&self, host: &str) -> Option<PfBadge> {
        let forwards = self
            .config_remotes
            .iter()
            .find(|r| r.host == host)
            .map(|r| r.forwards.as_slice())?;
        if forwards.is_empty() {
            return None;
        }
        let healths: Vec<ForwardHealth> = forwards
            .iter()
            .map(|f| {
                self.forward_health
                    .get(&ForwardKey::from_spec(host, f))
                    .copied()
                    .unwrap_or(ForwardHealth::Probing)
            })
            .collect();
        Some(PfBadge {
            count: forwards.len(),
            color: rollup_color(&healths),
        })
    }

    /// Drop health entries whose forward no longer exists in config (after a
    /// reload that removed forwards), so the map doesn't accrete dead keys.
    pub fn prune_forward_health(&mut self) {
        let valid: std::collections::HashSet<ForwardKey> = self
            .config_remotes
            .iter()
            .flat_map(|r| r.forwards.iter().map(|f| ForwardKey::from_spec(&r.host, f)))
            .collect();
        self.forward_health.retain(|k, _| valid.contains(k));
    }
```

- [ ] **Step 5: Call the prune after reload**

In `src/app/dispatch.rs`, immediately after line 633
(`self.state.config_remotes = new_remotes;`):

```rust
        self.state.prune_forward_health();
```

- [ ] **Step 6: Verify it builds**

Run: `cargo build`
Expected: builds clean (no test yet — `host_pf_badge` is exercised end-to-end in
Task 7's render test and manually).

- [ ] **Step 7: Commit**

```bash
git add src/model/state.rs src/app/dispatch.rs
git commit -m "feat(pf): forward_health map, host_pf_badge, reload prune"
```

---

## Task 4: Worker `Probe` op + listener Runner method (`port_forward_task.rs`)

Add `Op::Probe`, an `OpKind::Probe(ForwardKey, ForwardHealth)` result, a
`Runner::listening_ports` method, an `OpKind::host()` accessor, and the
classification logic.

**Files:**
- Modify: `src/app/port_forward_task.rs`
- Modify: `tests/unit/app/port_forward_task.rs` (extend `MockRunner`, add test)

- [ ] **Step 1: Write the failing test**

In `tests/unit/app/port_forward_task.rs`, first extend the imports at the top:

```rust
use std::collections::HashSet;
use crate::state::{ForwardHealth, ForwardKey};
use crate::app::port_forward_task::OpKind;
```

Add a `listening` field to `MockRunner`:

```rust
#[derive(Default, Clone)]
struct MockRunner {
    log: Arc<Mutex<Vec<String>>>,
    fail_master: Arc<Mutex<Vec<String>>>,
    listening: Arc<Mutex<Option<HashSet<u16>>>>,
}
```

Add the new trait method to `impl Runner for MockRunner`:

```rust
    fn listening_ports(&self) -> Option<HashSet<u16>> {
        self.listening.lock().unwrap().clone()
    }
```

Add the test:

```rust
#[test]
fn probe_classifies_by_mode_and_listeners() {
    let runner = MockRunner::default();
    *runner.listening.lock().unwrap() = Some(HashSet::from([8080u16])); // 8080 up, 1080 down
    let mut w = Worker::new(runner.clone());
    // Bring host "h" master up so the -R forward reads Presumed.
    w.handle(Op::Bootstrap { hosts: vec![("h".into(), vec![])] });

    let key = |mode, port| ForwardKey {
        host: "h".into(),
        mode,
        bind_addr: None,
        listen_port: port,
    };
    let results = w.handle(Op::Probe {
        items: vec![
            key(ForwardMode::Local, 8080),
            key(ForwardMode::Dynamic, 1080),
            key(ForwardMode::Remote, 9090),
        ],
    });

    let health = |i: usize| match &results[i].kind {
        OpKind::Probe(_, h) => *h,
        other => panic!("expected Probe kind, got {:?}", other),
    };
    assert_eq!(health(0), ForwardHealth::Up); // -L 8080 is listening
    assert_eq!(health(1), ForwardHealth::Down); // -D 1080 not listening
    assert_eq!(health(2), ForwardHealth::Presumed); // -R, master up
}

#[test]
fn probe_local_down_when_enumeration_unavailable() {
    let runner = MockRunner::default(); // listening = None
    let mut w = Worker::new(runner);
    let results = w.handle(Op::Probe {
        items: vec![ForwardKey {
            host: "h".into(),
            mode: ForwardMode::Local,
            bind_addr: None,
            listen_port: 8080,
        }],
    });
    match &results[0].kind {
        OpKind::Probe(_, h) => assert_eq!(*h, ForwardHealth::Probing),
        other => panic!("expected Probe kind, got {:?}", other),
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib probe_classifies`
Expected: FAIL — `Op::Probe`, `OpKind::Probe`, `listening_ports` don't exist.

- [ ] **Step 3: Add the `Op::Probe` variant**

In `src/app/port_forward_task.rs`, add to `pub enum Op` (after `StopHost`):

```rust
    /// Classify the liveness of each given forward. Enumerates local listeners
    /// once when any item is `-L`/`-D`.
    Probe { items: Vec<crate::state::ForwardKey> },
```

- [ ] **Step 4: Add the `OpKind::Probe` variant + `host()` accessor**

Add to `pub enum OpKind` (after `Exit(String)`):

```rust
    Probe(crate::state::ForwardKey, crate::state::ForwardHealth),
```

Add an accessor below the enum:

```rust
impl OpKind {
    /// The host this result pertains to.
    pub fn host(&self) -> &str {
        match self {
            OpKind::Master(h) | OpKind::Exit(h) => h,
            OpKind::Forward(h, _) | OpKind::Cancel(h, _) => h,
            OpKind::Probe(key, _) => &key.host,
        }
    }
}
```

- [ ] **Step 5: Add `listening_ports` to the `Runner` trait + `SshRunner`**

In the `pub trait Runner` block, add this method. It returns `Option` so the
worker can tell "couldn't enumerate" (→ `Probing`) apart from "enumerated, port
absent" (→ `Down`):

```rust
    fn listening_ports(&self) -> Option<std::collections::HashSet<u16>>;
```

In `impl Runner for SshRunner`, add:

```rust
    fn listening_ports(&self) -> Option<std::collections::HashSet<u16>> {
        crate::infra::listeners::local_listen_ports()
    }
```

- [ ] **Step 6: Handle `Op::Probe` in `Worker::handle`**

Add this arm to the `match op` in `handle` (after the `StopHost` arm):

```rust
            Op::Probe { items } => {
                use crate::config::ForwardMode;
                use crate::state::ForwardHealth;
                let needs_local = items
                    .iter()
                    .any(|k| matches!(k.mode, ForwardMode::Local | ForwardMode::Dynamic));
                let ports = if needs_local {
                    self.runner.listening_ports()
                } else {
                    Some(std::collections::HashSet::new())
                };
                items
                    .into_iter()
                    .map(|key| {
                        let health = match key.mode {
                            ForwardMode::Local | ForwardMode::Dynamic => match &ports {
                                Some(set) => {
                                    if set.contains(&key.listen_port) {
                                        ForwardHealth::Up
                                    } else {
                                        ForwardHealth::Down
                                    }
                                }
                                None => ForwardHealth::Probing, // couldn't enumerate
                            },
                            ForwardMode::Remote => {
                                if self.masters_up.contains(&key.host) {
                                    ForwardHealth::Presumed
                                } else {
                                    ForwardHealth::Down
                                }
                            }
                        };
                        OpResult {
                            kind: OpKind::Probe(key, health),
                            ok: true,
                            message: String::new(),
                        }
                    })
                    .collect()
            }
```

- [ ] **Step 7: Run tests to verify they pass**

Run: `cargo test --lib port_forward_task`
Expected: PASS — the two new tests plus the five existing worker tests.

- [ ] **Step 8: Commit**

```bash
git add src/app/port_forward_task.rs tests/unit/app/port_forward_task.rs
git commit -m "feat(pf): worker Probe op classifies forward liveness"
```

---

## Task 5: Route probe results into `forward_health` (`action`, `reduce.rs`, `mod.rs`)

A dedicated `Action::PfProbeResult` keeps probe handling separate from the
apply/cancel `PfTaskResult` path. The main-loop drain branches on result kind.

**Files:**
- Modify: `src/app/action/mod.rs` (new `Action` variant near line 134)
- Modify: `src/app/action/reduce.rs` (new arm near line 811)
- Modify: `src/app/mod.rs:541-554` (drain loop branch)

- [ ] **Step 1: Add the `Action` variant**

In `src/app/action/mod.rs`, after the `PfTaskResult { … }` variant (ends ~line 134):

```rust
    PfProbeResult {
        key: crate::state::ForwardKey,
        health: crate::state::ForwardHealth,
    },
```

- [ ] **Step 2: Handle it in the reducer**

In `src/app/action/reduce.rs`, add an arm right after the `Action::PfTaskResult`
arm (line 809-811), before `Action::None`:

```rust
        Action::PfProbeResult { key, health } => {
            state.forward_health.insert(key, health);
        }
```

- [ ] **Step 3: Branch the drain loop on result kind**

In `src/app/mod.rs`, replace the port-forward drain block (lines 541-554):

```rust
            // Drain results from the port-forward worker thread.
            while let Ok(r) = self.port_forward_rx.try_recv() {
                let host = match &r.kind {
                    crate::app::port_forward_task::OpKind::Master(h)
                    | crate::app::port_forward_task::OpKind::Exit(h) => h.clone(),
                    crate::app::port_forward_task::OpKind::Forward(h, _)
                    | crate::app::port_forward_task::OpKind::Cancel(h, _) => h.clone(),
                };
                self.dispatch(Action::PfTaskResult {
                    host,
                    op: r.kind,
                    ok: r.ok,
                    message: r.message,
                });
            }
```

with:

```rust
            // Drain results from the port-forward worker thread.
            while let Ok(r) = self.port_forward_rx.try_recv() {
                match r.kind {
                    crate::app::port_forward_task::OpKind::Probe(key, health) => {
                        self.dispatch(Action::PfProbeResult { key, health });
                    }
                    kind => {
                        let host = kind.host().to_string();
                        self.dispatch(Action::PfTaskResult {
                            host,
                            op: kind,
                            ok: r.ok,
                            message: r.message,
                        });
                    }
                }
            }
```

- [ ] **Step 4: Verify it builds + existing tests pass**

Run: `cargo build && cargo test --lib`
Expected: builds clean; all existing tests pass. The reducer arm is exercised
end-to-end; no isolated unit test (constructing a full `AppState` for one
`insert` is not worth it — covered by the manual check in the final task).

- [ ] **Step 5: Commit**

```bash
git add src/app/action/mod.rs src/app/action/reduce.rs src/app/mod.rs
git commit -m "feat(pf): route probe results into forward_health"
```

---

## Task 6: Dispatch a probe each refresh tick (`mod.rs`)

**Files:**
- Modify: `src/app/mod.rs:521-524` (call site) + a new helper method

- [ ] **Step 1: Add the `request_pf_probe` helper**

In `src/app/mod.rs`, add a method on `impl App` (near the other request helpers;
search for `fn request_refresh`):

```rust
    /// Ask the port-forward worker to re-classify every configured forward.
    /// No-op when nothing is configured (skips the `netstat`/`ss` spawn).
    fn request_pf_probe(&self) {
        let mut items = Vec::new();
        for r in &self.state.config_remotes {
            for f in &r.forwards {
                items.push(crate::state::ForwardKey::from_spec(&r.host, f));
            }
        }
        if items.is_empty() {
            return;
        }
        let _ = self
            .port_forward_tx
            .send(crate::app::port_forward_task::Op::Probe { items });
    }
```

- [ ] **Step 2: Call it on the refresh tick**

In `src/app/mod.rs`, in the refresh block (lines 521-524), add the probe call:

```rust
            if last_refresh.elapsed() >= REFRESH_INTERVAL {
                self.request_refresh();
                self.request_pf_probe();
                last_refresh = Instant::now();
            }
```

- [ ] **Step 3: Verify it builds**

Run: `cargo build && cargo clippy -- -D warnings`
Expected: builds clean, no clippy warnings.

- [ ] **Step 4: Manual smoke test**

Requires an SSH-reachable host configured as a remote with a forward. With one
configured, run `./target/debug/deck` and confirm no panic/hang on the 1s tick.
(Visual badge comes in Task 7; here we only confirm the probe loop is wired and
harmless.)

- [ ] **Step 5: Commit**

```bash
git add src/app/mod.rs
git commit -m "feat(pf): probe forward liveness each refresh tick"
```

---

## Task 7: Sidebar `⇄N` badge (`state.rs`, `sidebar.rs`)

Carry the badge on the `Header` item, compute it when building the layout, and
render it between the rule and the `[⟳]` button — reserving its width so the
right-aligned buttons don't move.

**Files:**
- Modify: `src/model/state.rs:220-224` (Header field) + `:946-953` (compute)
- Modify: `src/ui/sidebar.rs:286-302` (pass through) + `:384-444` (render)
- Modify: `tests/unit/ui/sidebar.rs` (badge width test)

- [ ] **Step 1: Write the failing test**

Append to `tests/unit/ui/sidebar.rs`:

```rust
#[test]
fn pf_badge_does_not_shift_right_aligned_buttons() {
    use crate::state::{HostStatus, PfBadge, PfBadgeColor};
    let theme = &crate::theme::THEMES[0];

    let mut without = Vec::new();
    let (recon_no, more_no) =
        super::render_group_header(&mut without, "@h", theme.teal, HostStatus::Connected, 60, theme, None);

    let mut with = Vec::new();
    let (recon_yes, more_yes) = super::render_group_header(
        &mut with,
        "@h",
        theme.teal,
        HostStatus::Connected,
        60,
        theme,
        Some(PfBadge { count: 2, color: PfBadgeColor::Healthy }),
    );

    // The badge eats into the dash run, so the buttons stay put.
    assert_eq!(recon_yes.start, recon_no.start, "reconnect button must not move");
    assert_eq!(more_yes.start, more_no.start, "more button must not move");

    // And the badge text is actually rendered.
    let rendered: String = with[0].spans.iter().map(|s| s.content.as_ref()).collect();
    assert!(rendered.contains("⇄2"), "badge text missing: {rendered:?}");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib pf_badge_does_not_shift`
Expected: FAIL — `render_group_header` takes 6 args, not 7.

- [ ] **Step 3: Add the `pf` field to `Header`**

In `src/model/state.rs`, in `SidebarItemData::Header` (lines 220-224):

```rust
    Header {
        host: String,
        host_idx: usize,
        status: HostStatus,
        pf: Option<PfBadge>,
    },
```

- [ ] **Step 4: Compute the badge when building the layout**

In `src/model/state.rs`, in `sidebar_layout`, update the `push_header` call
(lines 946-953) to include `pf`:

```rust
                    layout.push_header(
                        SidebarItemData::Header {
                            host: r.host.clone(),
                            host_idx,
                            status,
                            pf: self.host_pf_badge(&r.host),
                        },
                        1,
                    );
```

- [ ] **Step 5: Pass `pf` through the renderer**

In `src/ui/sidebar.rs`, update the `Header` match arm (lines 286-302):

```rust
            SidebarItemData::Header {
                host,
                host_idx,
                status,
                pf,
            } => {
                let accent = host_accent(ctx.theme, *host_idx);
                let line_idx = lines.len();
                let label = format!("@{host}");
                let (reconnect_range, more_range) = render_group_header(
                    &mut lines, &label, accent, *status, width, ctx.theme, *pf,
                );
                pending_hits.push((
                    line_idx,
                    reconnect_range,
                    host.clone(),
                    DividerButton::Reconnect,
                ));
                pending_hits.push((line_idx, more_range, host.clone(), DividerButton::More));
            }
```

- [ ] **Step 6: Render the badge in `render_group_header`**

In `src/ui/sidebar.rs`, replace the whole `render_group_header` function
(lines 384-444) with:

```rust
fn render_group_header(
    lines: &mut Vec<Line<'_>>,
    label: &str,
    accent: Color,
    status: HostStatus,
    width: usize,
    theme: &Theme,
    pf: Option<crate::state::PfBadge>,
) -> (std::ops::Range<usize>, std::ops::Range<usize>) {
    let label_text = label.trim_start().to_string();
    let leading = " ";
    let leading_w = leading.width();
    let label_w = label_text.as_str().width();
    let spacer_w = 1;
    let button_w = 3; // "[⟳]" / "[…]"
    let gap = 1; // space before each button
    // Right side of the divider: gap [⟳] gap […]
    let buttons_w = gap + button_w + gap + button_w;

    // Optional port-forward badge: " " + "⇄N", sitting between the rule and the
    // reconnect button. Reserve its width so the right-aligned buttons hold.
    let badge_text = pf.map(|b| format!("\u{21c4}{}", b.count));
    let badge_w = badge_text.as_ref().map(|s| gap + s.as_str().width()).unwrap_or(0);
    let badge_fg = pf.map(|b| match b.color {
        crate::state::PfBadgeColor::Healthy => theme.green,
        crate::state::PfBadgeColor::Degraded => theme.pink,
        crate::state::PfBadgeColor::Probing => theme.yellow,
    });

    let rule_w = width
        .saturating_sub(leading_w)
        .saturating_sub(label_w)
        .saturating_sub(spacer_w)
        .saturating_sub(badge_w)
        .saturating_sub(buttons_w);
    let rule = "\u{2500}".repeat(rule_w);

    // Tint the reconnect glyph by connection status; the "more" button keeps
    // the per-host accent.
    let reconnect_fg = match status {
        HostStatus::Connected => theme.green,
        HostStatus::Connecting => theme.yellow,
        HostStatus::Unreachable => theme.pink,
    };

    let mut spans = vec![
        Span::styled(leading, Style::default().bg(theme.bg)),
        Span::styled(
            label_text,
            Style::default()
                .fg(accent)
                .bg(theme.bg)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" ", Style::default().bg(theme.bg)),
        Span::styled(rule, Style::default().fg(accent).bg(theme.bg)),
    ];
    if let (Some(text), Some(fg)) = (badge_text, badge_fg) {
        spans.push(Span::styled(" ", Style::default().bg(theme.bg)));
        spans.push(Span::styled(text, Style::default().fg(fg).bg(theme.bg)));
    }
    spans.push(Span::styled(" ", Style::default().bg(theme.bg)));
    spans.push(Span::styled("[\u{27f3}]", Style::default().fg(reconnect_fg).bg(theme.bg)));
    spans.push(Span::styled(" ", Style::default().bg(theme.bg)));
    spans.push(Span::styled("[\u{2026}]", Style::default().fg(accent).bg(theme.bg)));

    lines.push(pad_line(spans, theme.bg, width));

    // Cell ranges of the two buttons within this rendered line.
    let reconnect_x = leading_w + label_w + spacer_w + rule_w + badge_w + gap;
    let more_x = reconnect_x + button_w + gap;
    (
        reconnect_x..(reconnect_x + button_w),
        more_x..(more_x + button_w),
    )
}
```

- [ ] **Step 7: Run test to verify it passes**

Run: `cargo test --lib pf_badge_does_not_shift`
Expected: PASS.

- [ ] **Step 8: Build, lint, and full test pass**

Run: `cargo clippy -- -D warnings && cargo test --lib`
Expected: clean, all pass.

- [ ] **Step 9: Commit**

```bash
git add src/model/state.rs src/ui/sidebar.rs tests/unit/ui/sidebar.rs
git commit -m "feat(pf): per-host ⇄N liveness badge on the divider"
```

---

## Task 8: Per-forward health dots in the overlay (`port_forward.rs`, `render.rs`)

**Files:**
- Modify: `src/ui/overlays/port_forward.rs` (`draw_port_forward` + `draw_list`)
- Modify: `src/app/render.rs:40` + `:368-374` (pass `forward_health`)

- [ ] **Step 1: Add imports to the overlay module**

At the top of `src/ui/overlays/port_forward.rs`, extend the `crate::state`
import and add `HashMap`:

```rust
use std::collections::HashMap;
use crate::state::{ForwardHealth, ForwardKey, PfAddForm, PfField, PortForwardOverlay};
```

- [ ] **Step 2: Thread health through `draw_port_forward`**

Change the `draw_port_forward` signature (line 16) to accept the health map,
and forward it to `draw_list`:

```rust
pub fn draw_port_forward(
    buf: &mut Buffer,
    area: Rect,
    overlay: &PortForwardOverlay,
    remotes: &[RemoteConfig],
    health: &HashMap<ForwardKey, ForwardHealth>,
    theme: &Theme,
) {
```

In its body, change the `draw_list` call (line 52):

```rust
        None => draw_list(buf, inner, forwards, overlay, health, theme),
```

- [ ] **Step 3: Render the dots in `draw_list`**

Change the `draw_list` signature (line 57) and the row-building loop. Replace
lines 57-83 (the signature through the `forwards` loop body) with:

```rust
fn draw_list(
    buf: &mut Buffer,
    area: Rect,
    forwards: &[ForwardSpec],
    overlay: &PortForwardOverlay,
    health: &HashMap<ForwardKey, ForwardHealth>,
    theme: &Theme,
) {
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::raw(""));
    if forwards.is_empty() {
        lines.push(Line::styled(
            "  (no forwards configured \u{2014} press a to add)",
            Style::default().fg(theme.muted),
        ));
    } else {
        for (i, f) in forwards.iter().enumerate() {
            let marker = if i == overlay.selected { ">" } else { " " };
            let style = if i == overlay.selected {
                Style::default().fg(theme.accent).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.text)
            };
            let h = health
                .get(&ForwardKey::from_spec(&overlay.host, f))
                .copied()
                .unwrap_or(ForwardHealth::Probing);
            let (dot, dot_fg) = match h {
                ForwardHealth::Up => ("\u{25cf}", theme.green),       // ●
                ForwardHealth::Down => ("\u{2715}", theme.pink),      // ✕
                ForwardHealth::Presumed => ("\u{25cb}", theme.dim),   // ○
                ForwardHealth::Probing => ("\u{00b7}", theme.muted),  // ·
            };
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(dot, Style::default().fg(dot_fg)),
                Span::raw(" "),
                Span::styled(format!("{} {}", marker, format_forward(f)), style),
            ]));
        }
    }
```

(The remainder of `draw_list` — the blank line, status line, and hint bar from
line 85 onward — is unchanged.)

- [ ] **Step 4: Pass `forward_health` from the render layer**

In `src/app/render.rs`, near line 40 where `config_remotes` is bound, add:

```rust
        let forward_health = s.forward_health.clone();
```

Then update the `draw_port_forward` call (lines 368-374) to pass it:

```rust
                crate::ui::overlays::port_forward::draw_port_forward(
                    frame.buffer_mut(),
                    pf_area,
                    overlay,
                    &config_remotes,
                    &forward_health,
                    theme,
                );
```

- [ ] **Step 5: Build, lint, full test pass**

Run: `cargo clippy -- -D warnings && cargo test --lib`
Expected: clean, all pass.

- [ ] **Step 6: Commit**

```bash
git add src/ui/overlays/port_forward.rs src/app/render.rs
git commit -m "feat(pf): per-forward health dots in the overlay"
```

---

## Final verification

- [ ] **Step 1: Full build + lint + tests**

Run: `cargo build && cargo clippy -- -D warnings && cargo test`
Expected: clean build, no warnings, all tests green.

- [ ] **Step 2: Manual end-to-end (needs an SSH-reachable host)**

Configure a remote in `~/.config/deck/config.json` with a working `-L` forward,
e.g.:

```json
{ "remotes": [{ "host": "<your-host>",
  "forwards": [{ "mode": "local", "listen_port": 8080,
                 "target_host": "localhost", "target_port": 80 }] }] }
```

Then in `./target/release/deck` (build with `cargo build --release` first):

1. Divider for the host shows `⇄1` in **green** once the forward is up.
2. Open the overlay (`f` on a remote row, or click `[…]` → Port Forward): the
   forward row shows a green `●`.
3. From another terminal, kill the master:
   `ssh -O exit -o ControlPath=~/.ssh/cm-%r@%h:%p <your-host>`.
   Within ~1s the badge flips **pink** and the overlay dot becomes `✕`.
4. Add a `-R` forward → its dot is `○` (Presumed) and, if it's the only forward,
   the badge is green.
5. A host with no forwards shows **no badge**.

- [ ] **Step 3: Confirm no regression in existing port-forward behavior**

Add and delete a forward through the overlay; confirm apply/cancel still work and
the badge/​dots update on the next tick.

---

## Self-review notes

Coverage against the spec:
- Listener enumeration (non-intrusive, `None` on failure) → Task 1.
- `ForwardHealth`/`ForwardKey`/badge rollup → Task 2.
- `forward_health` storage + per-host badge + reload prune → Task 3.
- Worker probe + `-L`/`-D` listener check + `-R` presumed + enumeration-failure
  → `Probing` → Task 4.
- Result routing → Task 5.
- 1s cadence, skip when no forwards → Task 6.
- Per-section `⇄N` badge, width-reserved, color rollup, hidden when zero → Task 7.
- Per-forward overlay dots → Task 8.

Type consistency: `ForwardKey`, `ForwardHealth`, `PfBadge`, `PfBadgeColor`,
`rollup_color`, `host_pf_badge`, `Op::Probe`, `OpKind::Probe`, `OpKind::host`,
`Action::PfProbeResult`, `request_pf_probe`, `Runner::listening_ports` are used
with identical names/signatures across the tasks that define and consume them.
