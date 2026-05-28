# Port Forward Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add per-host SSH port forwards (L/R/D) configurable from a `[…]` button on the sidebar's remote-host divider, persisted to `config.json`, applied eagerly at startup and immediately on UI edit.

**Architecture:** Pure infra builders (`infra/port_forward.rs`) + a worker thread (`app/port_forward_task.rs`) that owns SSH process lifecycle. UI lives in `ui/overlays/port_forward.rs` and a small render+hit-test addition to `ui/sidebar.rs`. Reducer dispatches `Action::Pf*` variants; worker results come back as `Action::PfTaskResult`. Hot-reload diffs old vs new forwards and emits the same worker commands.

**Tech Stack:** Rust, ratatui, crossterm, serde, `std::thread` + `std::sync::mpsc` (no new dependencies).

**Spec reference:** `docs/superpowers/specs/2026-05-28-port-forward-design.md`

---

## File map

| Path | Status | Responsibility |
|---|---|---|
| `src/model/config.rs` | MODIFY | `ForwardMode`, `ForwardSpec`, `RemoteConfig.forwards`, `to_ssh_flag()` |
| `src/model/state.rs` | MODIFY | `MenuKind::HostDivider`, `PortForwardOverlay`, `PfAddForm`, `PfField`, `DividerHit`; `OverlayState.port_forward`; `AppState.divider_hits` |
| `src/infra/port_forward.rs` | NEW | Pure `Command` builders: master, forward, cancel, exit |
| `src/app/port_forward_task.rs` | NEW | Worker thread + `Runner` trait + command channel + per-host master tracking |
| `src/app/mod.rs` | MODIFY | Hold worker `Sender`; spawn worker on construct |
| `src/app/action/mod.rs` | MODIFY | New `Action::OpenHostDividerMenu`, `OpenPortForward`, `PfFocusUp/Down`, `PfAddOpen`, `PfAddCancel`, `PfAddFieldNext/Prev`, `PfAddInput(char)`, `PfAddBackspace`, `PfAddModeLeft/Right`, `PfAddSubmit`, `PfDelete`, `PfClose`, `PfTaskResult { host, op, ok, message }` |
| `src/app/action/mouse.rs` | MODIFY | Hit-test against `divider_hits` before `focus_at_row()` |
| `src/app/action/keyboard.rs` | MODIFY | `f` shortcut while remote-session focus; overlay key routing |
| `src/app/action/reduce.rs` | MODIFY | Reducer arms for all `Pf*` actions; `OpenHostDividerMenu`; `PfTaskResult` updates `overlay.status` and config |
| `src/app/dispatch.rs` | MODIFY | Side-effect routing: `add_forward`/`cancel_forward` → worker channel; extend `reload_config` to diff forwards |
| `src/ui/sidebar.rs` | MODIFY | `render_group_header` emits `[…]` and returns `DividerHit`; renderer collects hits into `state.divider_hits` |
| `src/ui/overlays/mod.rs` | NEW (or modify existing) | Re-export port-forward overlay drawer |
| `src/ui/overlays/port_forward.rs` | NEW | `draw_port_forward(...)` — list view and add subform |
| `src/ui/render.rs` (or main render dispatcher) | MODIFY | Draw port-forward overlay when `state.overlay.port_forward.is_some()` |
| `src/main.rs` | MODIFY | Spawn worker; call `port_forward_task::bootstrap` for hosts with forwards |
| `tests/unit/model/config.rs` | MODIFY | `forwards` (de)serialization, legacy compat |
| `tests/unit/infra/port_forward.rs` | NEW | Command builder args for L/R/D and edge cases |
| `tests/unit/app/port_forward_task.rs` | NEW | Worker ordering: master before forward; idempotent re-add |
| `tests/unit/app/action/reduce.rs` | MODIFY | `Pf*` action reducer arms |

---

## Task 1: Data model — `ForwardMode`, `ForwardSpec`, `to_ssh_flag()`

**Files:**
- Modify: `src/model/config.rs`
- Test: `tests/unit/model/config.rs`

- [ ] **Step 1.1: Write failing tests for `ForwardSpec::to_ssh_flag()` and serde roundtrip**

Append to `tests/unit/model/config.rs`:

```rust
use crate::config::{Config, ForwardMode, ForwardSpec, RemoteConfig};

#[test]
fn forward_spec_local_to_flag_with_bind() {
    let spec = ForwardSpec {
        mode: ForwardMode::Local,
        bind_addr: Some("127.0.0.1".into()),
        listen_port: 8080,
        target_host: Some("example.com".into()),
        target_port: Some(80),
    };
    assert_eq!(spec.to_ssh_flag(), "-L 127.0.0.1:8080:example.com:80");
}

#[test]
fn forward_spec_local_to_flag_no_bind() {
    let spec = ForwardSpec {
        mode: ForwardMode::Local,
        bind_addr: None,
        listen_port: 8080,
        target_host: Some("example.com".into()),
        target_port: Some(80),
    };
    assert_eq!(spec.to_ssh_flag(), "-L 8080:example.com:80");
}

#[test]
fn forward_spec_remote_to_flag() {
    let spec = ForwardSpec {
        mode: ForwardMode::Remote,
        bind_addr: Some("0.0.0.0".into()),
        listen_port: 9090,
        target_host: Some("localhost".into()),
        target_port: Some(5432),
    };
    assert_eq!(spec.to_ssh_flag(), "-R 0.0.0.0:9090:localhost:5432");
}

#[test]
fn forward_spec_dynamic_to_flag() {
    let spec = ForwardSpec {
        mode: ForwardMode::Dynamic,
        bind_addr: None,
        listen_port: 1080,
        target_host: None,
        target_port: None,
    };
    assert_eq!(spec.to_ssh_flag(), "-D 1080");
}

#[test]
fn forward_spec_dynamic_with_bind_to_flag() {
    let spec = ForwardSpec {
        mode: ForwardMode::Dynamic,
        bind_addr: Some("127.0.0.1".into()),
        listen_port: 1080,
        target_host: None,
        target_port: None,
    };
    assert_eq!(spec.to_ssh_flag(), "-D 127.0.0.1:1080");
}

#[test]
fn remote_config_without_forwards_field_deserializes() {
    let json = r#"{ "host": "server-1" }"#;
    let r: RemoteConfig = serde_json::from_str(json).unwrap();
    assert_eq!(r.host, "server-1");
    assert!(r.forwards.is_empty());
}

#[test]
fn remote_config_empty_forwards_not_emitted() {
    let r = RemoteConfig { host: "server-1".into(), forwards: vec![] };
    let s = serde_json::to_string(&r).unwrap();
    assert!(!s.contains("forwards"), "empty forwards should be skipped: {}", s);
}

#[test]
fn remote_config_forwards_roundtrip() {
    let r = RemoteConfig {
        host: "h".into(),
        forwards: vec![ForwardSpec {
            mode: ForwardMode::Local,
            bind_addr: None,
            listen_port: 8080,
            target_host: Some("localhost".into()),
            target_port: Some(80),
        }],
    };
    let s = serde_json::to_string(&r).unwrap();
    let parsed: RemoteConfig = serde_json::from_str(&s).unwrap();
    assert_eq!(parsed, r);
}
```

- [ ] **Step 1.2: Run tests, verify they fail**

```
cargo test --lib -- forward_spec remote_config_without remote_config_empty remote_config_forwards
```
Expected: compile errors (`ForwardSpec`, `ForwardMode` not found).

- [ ] **Step 1.3: Add the types and impl to `src/model/config.rs`**

Replace the `RemoteConfig` struct (around line 22) with:

```rust
/// A remote host whose tmux sessions deck should surface alongside local ones.
/// The host string must resolve to an entry in the user's `~/.ssh/config`
/// (or a directly-resolvable hostname); deck shells out to `ssh <host> ...`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteConfig {
    pub host: String,
    /// Persisted SSH port forwards for this host. Applied at deck startup
    /// (eager) and immediately on UI edits via `ssh -O forward/cancel`
    /// against the host's ControlMaster.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub forwards: Vec<ForwardSpec>,
}

/// One SSH port-forward rule. Maps to a single `-L`, `-R`, or `-D` flag.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ForwardSpec {
    pub mode: ForwardMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bind_addr: Option<String>,
    pub listen_port: u16,
    /// Local/Remote: required (target endpoint on the other side).
    /// Dynamic: must be `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_port: Option<u16>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ForwardMode {
    Local,
    Remote,
    Dynamic,
}

impl ForwardSpec {
    /// Render this rule as the corresponding `ssh -L/-R/-D` argument
    /// pair. Caller splits on spaces to feed `Command::arg()`.
    pub fn to_ssh_flag(&self) -> String {
        let flag = match self.mode {
            ForwardMode::Local => "-L",
            ForwardMode::Remote => "-R",
            ForwardMode::Dynamic => "-D",
        };
        let bind_prefix = match &self.bind_addr {
            Some(b) => format!("{}:", b),
            None => String::new(),
        };
        match self.mode {
            ForwardMode::Dynamic => format!("{} {}{}", flag, bind_prefix, self.listen_port),
            ForwardMode::Local | ForwardMode::Remote => {
                let th = self.target_host.as_deref().unwrap_or("");
                let tp = self.target_port.unwrap_or(0);
                format!("{} {}{}:{}:{}", flag, bind_prefix, self.listen_port, th, tp)
            }
        }
    }
}
```

- [ ] **Step 1.4: Run tests, verify they pass**

```
cargo test --lib -- forward_spec remote_config
```
Expected: all 8 tests pass.

- [ ] **Step 1.5: Commit**

```
git add src/model/config.rs tests/unit/model/config.rs
git commit -m "Add ForwardSpec/ForwardMode and RemoteConfig.forwards"
```

---

## Task 2: SSH command builders (`infra/port_forward.rs`)

**Files:**
- Create: `src/infra/port_forward.rs`
- Modify: `src/infra/mod.rs` (re-export)
- Create: `tests/unit/infra/port_forward.rs`

- [ ] **Step 2.1: Add tests file**

Create `tests/unit/infra/port_forward.rs`:

```rust
use crate::config::{ForwardMode, ForwardSpec};
use crate::infra::port_forward::{
    build_cancel_cmd, build_exit_cmd, build_forward_cmd, build_master_cmd, ssh_args_for_host,
};

fn args_of(cmd: &std::process::Command) -> Vec<String> {
    cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect()
}

#[test]
fn ssh_args_uses_shared_control_options() {
    let args = ssh_args_for_host("server-1");
    // ControlMaster=auto / ControlPath / ControlPersist must be present so
    // the port-forward worker shares the master with the interactive path.
    let joined = args.join(" ");
    assert!(joined.contains("ControlMaster=auto"), "missing ControlMaster: {}", joined);
    assert!(joined.contains("ControlPath="), "missing ControlPath: {}", joined);
    assert!(joined.contains("ControlPersist="), "missing ControlPersist: {}", joined);
    assert!(args.contains(&"server-1".to_string()), "host missing from args");
}

#[test]
fn master_cmd_uses_fN_and_no_forwards() {
    let cmd = build_master_cmd("server-1");
    let args = args_of(&cmd);
    let joined = args.join(" ");
    assert!(joined.contains("-f"), "expected -f: {}", joined);
    assert!(joined.contains("-N"), "expected -N: {}", joined);
    // master cmd carries no -L/-R/-D
    assert!(!joined.contains(" -L "));
    assert!(!joined.contains(" -R "));
    assert!(!joined.contains(" -D "));
}

#[test]
fn forward_cmd_local() {
    let spec = ForwardSpec {
        mode: ForwardMode::Local,
        bind_addr: None,
        listen_port: 8080,
        target_host: Some("localhost".into()),
        target_port: Some(80),
    };
    let cmd = build_forward_cmd("h", &spec);
    let args = args_of(&cmd);
    assert!(args.iter().any(|a| a == "-O"), "missing -O");
    let oi = args.iter().position(|a| a == "-O").unwrap();
    assert_eq!(args[oi + 1], "forward");
    assert!(args.contains(&"-L".into()));
    assert!(args.contains(&"8080:localhost:80".into()));
}

#[test]
fn cancel_cmd_uses_O_cancel() {
    let spec = ForwardSpec {
        mode: ForwardMode::Dynamic,
        bind_addr: None,
        listen_port: 1080,
        target_host: None,
        target_port: None,
    };
    let cmd = build_cancel_cmd("h", &spec);
    let args = args_of(&cmd);
    let oi = args.iter().position(|a| a == "-O").unwrap();
    assert_eq!(args[oi + 1], "cancel");
    assert!(args.contains(&"-D".into()));
    assert!(args.contains(&"1080".into()));
}

#[test]
fn exit_cmd_uses_O_exit() {
    let cmd = build_exit_cmd("h");
    let args = args_of(&cmd);
    let oi = args.iter().position(|a| a == "-O").unwrap();
    assert_eq!(args[oi + 1], "exit");
}
```

- [ ] **Step 2.2: Implement `src/infra/port_forward.rs`**

```rust
//! Pure builders for the ssh subcommands used to manage port forwards
//! against a host's ControlMaster. No IO happens here; callers spawn
//! the returned `Command`s on their own threads.
//!
//! All builders pass the same `-o ControlMaster=auto -o ControlPath=…
//! -o ControlPersist=…` block so this worker and `app::remote_spawn`
//! share the same master socket per host.

use std::process::Command;

use crate::config::ForwardSpec;

/// The common ssh argument block: control options + host. Keep in sync
/// with `app/remote_spawn.rs` so both code paths reach the same master.
pub fn ssh_args_for_host(host: &str) -> Vec<String> {
    vec![
        "-o".into(), "ControlMaster=auto".into(),
        "-o".into(), "ControlPath=~/.ssh/cm-%r@%h:%p".into(),
        "-o".into(), "ControlPersist=10m".into(),
        "-o".into(), "BatchMode=yes".into(),
        host.into(),
    ]
}

fn ssh_with(host: &str, leading: &[&str]) -> Command {
    let mut c = Command::new("ssh");
    for a in leading {
        c.arg(a);
    }
    for a in ssh_args_for_host(host) {
        c.arg(a);
    }
    c
}

/// `ssh -fN <opts> <host>` — fork master into background, no remote command.
/// Returns immediately once the master is ready.
pub fn build_master_cmd(host: &str) -> Command {
    ssh_with(host, &["-f", "-N"])
}

fn spec_flag_pair(spec: &ForwardSpec) -> (String, String) {
    // to_ssh_flag returns e.g. "-L 8080:localhost:80"; split into ("-L", value).
    let s = spec.to_ssh_flag();
    let mut it = s.splitn(2, ' ');
    let flag = it.next().unwrap_or("").to_string();
    let value = it.next().unwrap_or("").to_string();
    (flag, value)
}

/// `ssh -O forward -L 8080:host:80 <opts> <host>` — add a forward to
/// the existing master. Fails with non-zero exit if master isn't up.
pub fn build_forward_cmd(host: &str, spec: &ForwardSpec) -> Command {
    let (flag, value) = spec_flag_pair(spec);
    let mut c = Command::new("ssh");
    c.arg("-O").arg("forward").arg(flag).arg(value);
    for a in ssh_args_for_host(host) {
        c.arg(a);
    }
    c
}

/// `ssh -O cancel -L 8080:host:80 <opts> <host>` — remove a forward.
pub fn build_cancel_cmd(host: &str, spec: &ForwardSpec) -> Command {
    let (flag, value) = spec_flag_pair(spec);
    let mut c = Command::new("ssh");
    c.arg("-O").arg("cancel").arg(flag).arg(value);
    for a in ssh_args_for_host(host) {
        c.arg(a);
    }
    c
}

/// `ssh -O exit <opts> <host>` — tear down the master entirely.
pub fn build_exit_cmd(host: &str) -> Command {
    ssh_with(host, &["-O", "exit"])
}

#[cfg(test)]
#[path = "../../tests/unit/infra/port_forward.rs"]
mod tests;
```

- [ ] **Step 2.3: Wire module in `src/infra/mod.rs`**

Read `src/infra/mod.rs` first; add `pub mod port_forward;` next to the other `pub mod` lines.

- [ ] **Step 2.4: Run tests**

```
cargo test --lib -- port_forward
```
Expected: 5 tests pass.

- [ ] **Step 2.5: Commit**

```
git add src/infra/port_forward.rs src/infra/mod.rs tests/unit/infra/port_forward.rs
git commit -m "Add ssh command builders for port-forward worker"
```

---

## Task 3: `diff_forwards` helper for hot-reload

**Files:**
- Modify: `src/model/config.rs` (add `diff_forwards` and `ForwardOp`)
- Modify: `tests/unit/model/config.rs`

- [ ] **Step 3.1: Write failing tests**

Append to `tests/unit/model/config.rs`:

```rust
use crate::config::{diff_forwards, ForwardOp};

fn fwd(port: u16) -> ForwardSpec {
    ForwardSpec {
        mode: ForwardMode::Local,
        bind_addr: None,
        listen_port: port,
        target_host: Some("localhost".into()),
        target_port: Some(80),
    }
}

#[test]
fn diff_forwards_added() {
    let old: Vec<ForwardSpec> = vec![];
    let new = vec![fwd(8080)];
    let ops = diff_forwards(&old, &new);
    assert_eq!(ops.len(), 1);
    assert!(matches!(&ops[0], ForwardOp::Add(s) if s.listen_port == 8080));
}

#[test]
fn diff_forwards_removed() {
    let old = vec![fwd(8080)];
    let new: Vec<ForwardSpec> = vec![];
    let ops = diff_forwards(&old, &new);
    assert_eq!(ops.len(), 1);
    assert!(matches!(&ops[0], ForwardOp::Cancel(s) if s.listen_port == 8080));
}

#[test]
fn diff_forwards_unchanged_emits_nothing() {
    let v = vec![fwd(8080)];
    let ops = diff_forwards(&v, &v);
    assert!(ops.is_empty());
}

#[test]
fn diff_forwards_mixed() {
    let old = vec![fwd(8080), fwd(9090)];
    let new = vec![fwd(8080), fwd(7070)];
    let ops = diff_forwards(&old, &new);
    assert_eq!(ops.len(), 2);
    assert!(ops.iter().any(|o| matches!(o, ForwardOp::Cancel(s) if s.listen_port == 9090)));
    assert!(ops.iter().any(|o| matches!(o, ForwardOp::Add(s) if s.listen_port == 7070)));
}
```

- [ ] **Step 3.2: Run tests — they should fail to compile**

```
cargo test --lib -- diff_forwards
```
Expected: compile error (`diff_forwards`, `ForwardOp` not found).

- [ ] **Step 3.3: Implement `diff_forwards` in `src/model/config.rs`**

Append (near the other forward types):

```rust
/// Difference between two `Vec<ForwardSpec>` slices: which to add and
/// which to cancel. Order-insensitive; equal specs (by all fields) are
/// considered the same. Used by both UI edits (single-item ops) and
/// hot-reload (bulk).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForwardOp {
    Add(ForwardSpec),
    Cancel(ForwardSpec),
}

pub fn diff_forwards(old: &[ForwardSpec], new: &[ForwardSpec]) -> Vec<ForwardOp> {
    let mut ops = Vec::new();
    for o in old {
        if !new.contains(o) {
            ops.push(ForwardOp::Cancel(o.clone()));
        }
    }
    for n in new {
        if !old.contains(n) {
            ops.push(ForwardOp::Add(n.clone()));
        }
    }
    ops
}
```

- [ ] **Step 3.4: Run tests, verify pass**

```
cargo test --lib -- diff_forwards
```
Expected: 4 tests pass.

- [ ] **Step 3.5: Commit**

```
git add src/model/config.rs tests/unit/model/config.rs
git commit -m "Add diff_forwards / ForwardOp for hot-reload bulk diffs"
```

---

## Task 4: `PfAddForm::validate()` + form types

**Files:**
- Modify: `src/model/state.rs` (add `PfAddForm`, `PfField`, `FormError`, and `PortForwardOverlay` skeleton)
- Modify: `tests/unit/model/state.rs`

- [ ] **Step 4.1: Write failing tests**

Append to `tests/unit/model/state.rs`:

```rust
use crate::config::{ForwardMode, ForwardSpec};
use crate::state::{PfAddForm, PfField, PfFormError};

fn blank_form() -> PfAddForm {
    PfAddForm {
        mode: ForwardMode::Local,
        focus: PfField::ListenPort,
        bind_addr: String::new(),
        listen_port: String::new(),
        target_host: String::new(),
        target_port: String::new(),
    }
}

#[test]
fn validate_local_ok() {
    let mut f = blank_form();
    f.listen_port = "8080".into();
    f.target_host = "localhost".into();
    f.target_port = "80".into();
    let spec = f.validate().expect("should validate");
    assert_eq!(spec.listen_port, 8080);
    assert_eq!(spec.target_host.as_deref(), Some("localhost"));
    assert_eq!(spec.target_port, Some(80));
    assert_eq!(spec.bind_addr, None);
}

#[test]
fn validate_local_missing_target_host() {
    let mut f = blank_form();
    f.listen_port = "8080".into();
    f.target_port = "80".into();
    assert_eq!(f.validate(), Err(PfFormError::TargetHostRequired));
}

#[test]
fn validate_local_port_zero_rejected() {
    let mut f = blank_form();
    f.listen_port = "0".into();
    f.target_host = "h".into();
    f.target_port = "80".into();
    assert_eq!(f.validate(), Err(PfFormError::ListenPortRange));
}

#[test]
fn validate_local_port_non_numeric_rejected() {
    let mut f = blank_form();
    f.listen_port = "abc".into();
    f.target_host = "h".into();
    f.target_port = "80".into();
    assert_eq!(f.validate(), Err(PfFormError::ListenPortRange));
}

#[test]
fn validate_dynamic_clears_target() {
    let mut f = blank_form();
    f.mode = ForwardMode::Dynamic;
    f.listen_port = "1080".into();
    f.target_host = "stale".into();
    f.target_port = "999".into();
    let spec = f.validate().unwrap();
    assert_eq!(spec.target_host, None);
    assert_eq!(spec.target_port, None);
}

#[test]
fn validate_bind_addr_passthrough() {
    let mut f = blank_form();
    f.bind_addr = "127.0.0.1".into();
    f.listen_port = "8080".into();
    f.target_host = "h".into();
    f.target_port = "80".into();
    let spec = f.validate().unwrap();
    assert_eq!(spec.bind_addr.as_deref(), Some("127.0.0.1"));
}
```

- [ ] **Step 4.2: Run tests — should fail to compile**

```
cargo test --lib -- validate_
```

- [ ] **Step 4.3: Add types and `validate()` to `src/model/state.rs`**

Append (after `ExcludeEditorState`):

```rust
// --- Port forward overlay ---

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PfField {
    Mode,
    BindAddr,
    ListenPort,
    TargetHost,
    TargetPort,
}

#[derive(Debug, Clone)]
pub struct PfAddForm {
    pub mode: crate::config::ForwardMode,
    pub focus: PfField,
    pub bind_addr: String,
    pub listen_port: String,
    pub target_host: String,
    pub target_port: String,
    /// True while a validated spec is in flight to the worker. The
    /// form stays rendered (read-only) until `PfTaskResult` for this
    /// host's Forward op clears or fails the submission. Lazy
    /// persist: config is only written when the worker reports
    /// success.
    pub submitting: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PfFormError {
    ListenPortRange,
    TargetPortRange,
    TargetHostRequired,
}

impl PfFormError {
    pub fn message(&self) -> &'static str {
        match self {
            PfFormError::ListenPortRange => "listen_port must be 1-65535",
            PfFormError::TargetPortRange => "target_port must be 1-65535",
            PfFormError::TargetHostRequired => "target_host required for -L/-R",
        }
    }
}

impl PfAddForm {
    pub fn default_for(mode: crate::config::ForwardMode) -> Self {
        Self {
            mode,
            focus: PfField::ListenPort,
            bind_addr: String::new(),
            listen_port: String::new(),
            target_host: String::new(),
            target_port: String::new(),
            submitting: false,
        }
    }

    pub fn validate(&self) -> Result<crate::config::ForwardSpec, PfFormError> {
        use crate::config::{ForwardMode, ForwardSpec};
        let listen_port: u16 = self
            .listen_port
            .trim()
            .parse()
            .map_err(|_| PfFormError::ListenPortRange)?;
        if listen_port == 0 {
            return Err(PfFormError::ListenPortRange);
        }
        let bind_addr = if self.bind_addr.trim().is_empty() {
            None
        } else {
            Some(self.bind_addr.trim().to_string())
        };

        match self.mode {
            ForwardMode::Dynamic => Ok(ForwardSpec {
                mode: ForwardMode::Dynamic,
                bind_addr,
                listen_port,
                target_host: None,
                target_port: None,
            }),
            ForwardMode::Local | ForwardMode::Remote => {
                let target_host = self.target_host.trim();
                if target_host.is_empty() {
                    return Err(PfFormError::TargetHostRequired);
                }
                let target_port: u16 = self
                    .target_port
                    .trim()
                    .parse()
                    .map_err(|_| PfFormError::TargetPortRange)?;
                if target_port == 0 {
                    return Err(PfFormError::TargetPortRange);
                }
                Ok(ForwardSpec {
                    mode: self.mode,
                    bind_addr,
                    listen_port,
                    target_host: Some(target_host.to_string()),
                    target_port: Some(target_port),
                })
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct PortForwardOverlay {
    pub host: String,
    pub selected: usize,
    pub add_form: Option<PfAddForm>,
    pub status: Option<String>,
}
```

- [ ] **Step 4.4: Run tests, verify pass**

```
cargo test --lib -- validate_
```
Expected: 6 tests pass.

- [ ] **Step 4.5: Commit**

```
git add src/model/state.rs tests/unit/model/state.rs
git commit -m "Add PfAddForm + validate() + PortForwardOverlay skeleton"
```

---

## Task 5: AppState extensions — `MenuKind::HostDivider`, `OverlayState.port_forward`, `DividerHit`

**Files:**
- Modify: `src/model/state.rs`

- [ ] **Step 5.1: Extend `MenuKind`**

In `src/model/state.rs`, change the `MenuKind` enum (around line 90) and the constants block (around line 33):

```rust
const HOST_DIVIDER_MENU_ITEMS: &'static [&'static str] = &["Port Forward"];

#[derive(Debug, Clone)]
pub enum MenuKind {
    Session {
        focus: FocusTarget,
        items: &'static [&'static str],
    },
    Global,
    /// Click on the `[…]` button on a remote host divider. Single
    /// item today (`Port Forward`); extendable.
    HostDivider {
        host: String,
        items: &'static [&'static str],
    },
}

impl MenuKind {
    pub fn items(&self) -> &'static [&'static str] {
        match self {
            MenuKind::Session { items, .. } => items,
            MenuKind::Global => GLOBAL_MENU_ITEMS,
            MenuKind::HostDivider { items, .. } => items,
        }
    }
}

pub fn host_divider_menu_items() -> &'static [&'static str] {
    HOST_DIVIDER_MENU_ITEMS
}
```

- [ ] **Step 5.2: Add `DividerHit` and extend `OverlayState` + `AppState`**

In `src/model/state.rs`:

```rust
/// Click-region for the `[…]` button on a remote-host divider. The
/// sidebar renderer fills `divider_hits` after each render; mouse
/// hit-testing consults it before `focus_at_row()`.
#[derive(Debug, Clone)]
pub struct DividerHit {
    pub host: String,
    pub rect: Rect,
}
```

Extend `OverlayState`:

```rust
#[derive(Debug, Default)]
pub struct OverlayState {
    pub show_help: bool,
    pub confirm_kill: bool,
    pub renaming: Option<RenameState>,
    pub context_menu: Option<ContextMenu>,
    pub exclude_editor: Option<ExcludeEditorState>,
    pub new_session: Option<NewSessionState>,
    /// Port-forward overlay for a single host. See `PortForwardOverlay`.
    pub port_forward: Option<PortForwardOverlay>,
}
```

Add field to `AppState`:

```rust
    // ... existing fields ...
    /// Click-regions for divider `[…]` buttons, refilled by the sidebar
    /// renderer each frame. Read by mouse dispatch.
    pub divider_hits: Vec<DividerHit>,
```

Initialize in `AppState::new`:

```rust
        divider_hits: Vec::new(),
```

- [ ] **Step 5.3: Compile-check**

```
cargo check
```
Expected: passes (no usages yet).

- [ ] **Step 5.4: Commit**

```
git add src/model/state.rs
git commit -m "Extend state: MenuKind::HostDivider, OverlayState.port_forward, DividerHit"
```

---

## Task 6: Port-forward worker — `app/port_forward_task.rs`

**Files:**
- Create: `src/app/port_forward_task.rs`
- Modify: `src/app/mod.rs` (declare module)
- Create: `tests/unit/app/port_forward_task.rs`

- [ ] **Step 6.1: Write failing test**

Create `tests/unit/app/port_forward_task.rs`:

```rust
use std::sync::{Arc, Mutex};

use crate::app::port_forward_task::{Op, Runner, Worker};
use crate::config::{ForwardMode, ForwardSpec};

#[derive(Default, Clone)]
struct MockRunner {
    log: Arc<Mutex<Vec<String>>>,
    /// Hostnames whose master "command" should report failure.
    fail_master: Arc<Mutex<Vec<String>>>,
}

impl Runner for MockRunner {
    fn run_master(&self, host: &str) -> Result<(), String> {
        self.log.lock().unwrap().push(format!("master {}", host));
        if self.fail_master.lock().unwrap().iter().any(|h| h == host) {
            Err("mock master failed".into())
        } else {
            Ok(())
        }
    }
    fn run_forward(&self, host: &str, spec: &ForwardSpec) -> Result<(), String> {
        self.log
            .lock()
            .unwrap()
            .push(format!("forward {} {}", host, spec.listen_port));
        Ok(())
    }
    fn run_cancel(&self, host: &str, spec: &ForwardSpec) -> Result<(), String> {
        self.log
            .lock()
            .unwrap()
            .push(format!("cancel {} {}", host, spec.listen_port));
        Ok(())
    }
    fn run_exit(&self, host: &str) -> Result<(), String> {
        self.log.lock().unwrap().push(format!("exit {}", host));
        Ok(())
    }
}

fn spec(port: u16) -> ForwardSpec {
    ForwardSpec {
        mode: ForwardMode::Local,
        bind_addr: None,
        listen_port: port,
        target_host: Some("h".into()),
        target_port: Some(80),
    }
}

#[test]
fn add_forward_starts_master_first_time() {
    let runner = MockRunner::default();
    let mut w = Worker::new(runner.clone());
    let results = w.handle(Op::AddForward { host: "h1".into(), spec: spec(8080) });
    let log = runner.log.lock().unwrap().clone();
    assert_eq!(log, vec!["master h1", "forward h1 8080"]);
    assert!(results.iter().all(|r| r.ok));
}

#[test]
fn add_forward_second_time_skips_master() {
    let runner = MockRunner::default();
    let mut w = Worker::new(runner.clone());
    w.handle(Op::AddForward { host: "h1".into(), spec: spec(8080) });
    runner.log.lock().unwrap().clear();
    w.handle(Op::AddForward { host: "h1".into(), spec: spec(9090) });
    let log = runner.log.lock().unwrap().clone();
    assert_eq!(log, vec!["forward h1 9090"]);
}

#[test]
fn add_forward_master_failure_skips_forward() {
    let runner = MockRunner::default();
    runner.fail_master.lock().unwrap().push("h1".into());
    let mut w = Worker::new(runner.clone());
    let results = w.handle(Op::AddForward { host: "h1".into(), spec: spec(8080) });
    let log = runner.log.lock().unwrap().clone();
    assert_eq!(log, vec!["master h1"]);
    // First (and only) result should be the failed master op.
    assert!(!results[0].ok);
}

#[test]
fn cancel_forward_does_not_touch_master() {
    let runner = MockRunner::default();
    let mut w = Worker::new(runner.clone());
    w.handle(Op::AddForward { host: "h1".into(), spec: spec(8080) });
    runner.log.lock().unwrap().clear();
    w.handle(Op::CancelForward { host: "h1".into(), spec: spec(8080) });
    let log = runner.log.lock().unwrap().clone();
    assert_eq!(log, vec!["cancel h1 8080"]);
}

#[test]
fn bootstrap_orders_master_before_each_host_forwards() {
    let runner = MockRunner::default();
    let mut w = Worker::new(runner.clone());
    w.handle(Op::Bootstrap {
        hosts: vec![
            ("h1".into(), vec![spec(8080), spec(9090)]),
            ("h2".into(), vec![spec(7070)]),
        ],
    });
    let log = runner.log.lock().unwrap().clone();
    assert_eq!(
        log,
        vec![
            "master h1",
            "forward h1 8080",
            "forward h1 9090",
            "master h2",
            "forward h2 7070",
        ]
    );
}
```

- [ ] **Step 6.2: Implement `src/app/port_forward_task.rs`**

```rust
//! Port-forward worker. Owns SSH process lifecycle: per-host
//! ControlMaster bring-up and individual `-O forward / -O cancel`
//! calls. UI thread sends `Op` messages on a channel; the worker
//! returns `OpResult` per executed step.
//!
//! Split from `infra::port_forward` so the I/O-bearing logic (process
//! tracking, threading) is testable via the `Runner` trait without
//! shelling out to real `ssh`.

use std::collections::HashSet;
use std::sync::mpsc::{Receiver, Sender};
use std::thread;

use crate::config::ForwardSpec;

/// Commands the UI sends to the worker.
#[derive(Debug)]
pub enum Op {
    /// Bring up master + apply every spec, host-by-host, in given order.
    Bootstrap { hosts: Vec<(String, Vec<ForwardSpec>)> },
    AddForward { host: String, spec: ForwardSpec },
    CancelForward { host: String, spec: ForwardSpec },
    /// Tear down the host's master entirely (used when a host is removed
    /// from config via hot-reload).
    StopHost { host: String },
}

/// Identifier for what the result is reporting on. Mirrored on
/// `Action::PfTaskResult` so the reducer can pick the right place to
/// surface the message.
#[derive(Debug, Clone)]
pub enum OpKind {
    Master(String),
    Forward(String, ForwardSpec),
    Cancel(String, ForwardSpec),
    Exit(String),
}

#[derive(Debug, Clone)]
pub struct OpResult {
    pub kind: OpKind,
    pub ok: bool,
    pub message: String,
}

/// Indirection over actually shelling out — lets tests verify ordering
/// without spawning ssh.
pub trait Runner: Send + 'static {
    fn run_master(&self, host: &str) -> Result<(), String>;
    fn run_forward(&self, host: &str, spec: &ForwardSpec) -> Result<(), String>;
    fn run_cancel(&self, host: &str, spec: &ForwardSpec) -> Result<(), String>;
    fn run_exit(&self, host: &str) -> Result<(), String>;
}

/// The default Runner — actually shells out via `infra::port_forward`.
pub struct SshRunner;

impl Runner for SshRunner {
    fn run_master(&self, host: &str) -> Result<(), String> {
        let mut cmd = crate::infra::port_forward::build_master_cmd(host);
        run_blocking(&mut cmd)
    }
    fn run_forward(&self, host: &str, spec: &ForwardSpec) -> Result<(), String> {
        let mut cmd = crate::infra::port_forward::build_forward_cmd(host, spec);
        run_blocking(&mut cmd)
    }
    fn run_cancel(&self, host: &str, spec: &ForwardSpec) -> Result<(), String> {
        let mut cmd = crate::infra::port_forward::build_cancel_cmd(host, spec);
        run_blocking(&mut cmd)
    }
    fn run_exit(&self, host: &str) -> Result<(), String> {
        let mut cmd = crate::infra::port_forward::build_exit_cmd(host);
        run_blocking(&mut cmd)
    }
}

fn run_blocking(cmd: &mut std::process::Command) -> Result<(), String> {
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let out = cmd.output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        let msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(if msg.is_empty() {
            format!("exit {}", out.status)
        } else {
            msg
        })
    }
}

/// Pure command-handling core. Carries the per-host master-up set
/// across calls. `handle()` is sync; the public `spawn()` glues it
/// to an mpsc channel and a thread.
pub struct Worker<R: Runner> {
    runner: R,
    masters_up: HashSet<String>,
}

impl<R: Runner> Worker<R> {
    pub fn new(runner: R) -> Self {
        Self {
            runner,
            masters_up: HashSet::new(),
        }
    }

    pub fn handle(&mut self, op: Op) -> Vec<OpResult> {
        match op {
            Op::Bootstrap { hosts } => {
                let mut out = Vec::new();
                for (host, specs) in hosts {
                    let master_ok = self.ensure_master(&host, &mut out);
                    if !master_ok {
                        continue;
                    }
                    for spec in specs {
                        let r = self.runner.run_forward(&host, &spec);
                        out.push(result_from(OpKind::Forward(host.clone(), spec), r));
                    }
                }
                out
            }
            Op::AddForward { host, spec } => {
                let mut out = Vec::new();
                if !self.ensure_master(&host, &mut out) {
                    return out;
                }
                let r = self.runner.run_forward(&host, &spec);
                out.push(result_from(OpKind::Forward(host, spec), r));
                out
            }
            Op::CancelForward { host, spec } => {
                let r = self.runner.run_cancel(&host, &spec);
                vec![result_from(OpKind::Cancel(host, spec), r)]
            }
            Op::StopHost { host } => {
                let r = self.runner.run_exit(&host);
                self.masters_up.remove(&host);
                vec![result_from(OpKind::Exit(host), r)]
            }
        }
    }

    /// Bring the host's master up if not already. Returns true on success.
    /// Records the master attempt result in `out`.
    fn ensure_master(&mut self, host: &str, out: &mut Vec<OpResult>) -> bool {
        if self.masters_up.contains(host) {
            return true;
        }
        let r = self.runner.run_master(host);
        let ok = r.is_ok();
        out.push(result_from(OpKind::Master(host.to_string()), r));
        if ok {
            self.masters_up.insert(host.to_string());
        }
        ok
    }
}

fn result_from(kind: OpKind, r: Result<(), String>) -> OpResult {
    match r {
        Ok(()) => OpResult { kind, ok: true, message: String::new() },
        Err(message) => OpResult { kind, ok: false, message },
    }
}

/// Spawn a worker thread that reads `Op`s and forwards `OpResult`s.
/// Returns the channel sender. The thread runs until the sender is
/// dropped.
pub fn spawn(results: Sender<OpResult>) -> Sender<Op> {
    let (op_tx, op_rx): (Sender<Op>, Receiver<Op>) = std::sync::mpsc::channel();
    thread::Builder::new()
        .name("deck-port-forward".into())
        .spawn(move || {
            let mut worker = Worker::new(SshRunner);
            for op in op_rx {
                for r in worker.handle(op) {
                    if results.send(r).is_err() {
                        return;
                    }
                }
            }
        })
        .expect("port-forward worker thread");
    op_tx
}

#[cfg(test)]
#[path = "../../tests/unit/app/port_forward_task.rs"]
mod tests;
```

- [ ] **Step 6.3: Wire module in `src/app/mod.rs`**

Add `pub mod port_forward_task;` next to the other `pub mod` lines.

- [ ] **Step 6.4: Run tests**

```
cargo test --lib -- port_forward_task
```
Expected: 5 tests pass.

- [ ] **Step 6.5: Commit**

```
git add src/app/port_forward_task.rs src/app/mod.rs tests/unit/app/port_forward_task.rs
git commit -m "Add port-forward worker with mock-tested ordering"
```

---

## Task 7: Divider `[…]` button rendering + hit emission

**Files:**
- Modify: `src/ui/sidebar.rs`

- [ ] **Step 7.1: Change `render_group_header` signature**

Replace the existing `render_group_header` (sidebar.rs:314–348) with the version below. The function now reserves 4 columns at the right (` […]` — 1 space + 3 chars) and returns the cell range that holds `[…]` for the caller to record.

```rust
fn render_group_header(
    lines: &mut Vec<Line<'_>>,
    label: &str,
    accent: Color,
    width: usize,
    theme: &Theme,
) -> std::ops::Range<usize> {
    let label_text = label.trim_start().to_string();
    let leading = " ";
    let leading_w = leading.width();
    let label_w = label_text.as_str().width();
    let spacer_w = 1;
    let button_w = 3; // "[…]"
    let button_gap = 1; // space between dashes and button
    let rule_w = width
        .saturating_sub(leading_w)
        .saturating_sub(label_w)
        .saturating_sub(spacer_w)
        .saturating_sub(button_gap)
        .saturating_sub(button_w);
    let rule = "─".repeat(rule_w);

    lines.push(pad_line(
        vec![
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
            Span::styled(" ", Style::default().bg(theme.bg)),
            Span::styled("[…]", Style::default().fg(accent).bg(theme.bg)),
        ],
        theme.bg,
        width,
    ));

    // Cell range of "[…]" within this rendered line.
    let button_x = leading_w + label_w + spacer_w + rule_w + button_gap;
    button_x..(button_x + button_w)
}
```

- [ ] **Step 7.2: Find every caller and record the `DividerHit`**

Grep for `render_group_header(`:

```
grep -n "render_group_header" src/ui/sidebar.rs
```

For each call site (likely inside the main sidebar render loop), record the returned range into a `Vec<DividerHit>` that the function later writes to `state.divider_hits`. Locate the loop that iterates `SidebarLayout::items` for `Header { label, host_idx }`. Extract the host name (the existing label is `"  @<host>"`; strip the leading whitespace + `@` to recover the host).

Sketch (adapt to actual call structure):

```rust
let mut new_hits: Vec<DividerHit> = Vec::new();
// ... within the items loop:
SidebarItemKind::Header { label, host_idx } => {
    let accent = host_accent(theme, *host_idx);
    let range = render_group_header(&mut lines, label, accent, width as usize, theme);
    let host = label.trim_start().trim_start_matches('@').to_string();
    let y = current_y; // y coord of this line within the sidebar's frame
    let rect = ratatui::layout::Rect {
        x: sidebar_x + range.start as u16,
        y,
        width: (range.end - range.start) as u16,
        height: 1,
    };
    new_hits.push(DividerHit { host, rect });
}
// ... after the loop:
state.divider_hits = new_hits;
```

The exact integration depends on whether the sidebar render function takes `&mut AppState` or `&AppState`. If it takes `&AppState`, return `Vec<DividerHit>` from the function and have the caller assign. Look at the existing render entry point (likely `app::render` or `ui::sidebar::draw_sidebar`) and follow the existing pattern for `banner_upgrade_bounds` (`AppState` field assigned during render).

- [ ] **Step 7.3: Add `divider_hits` clearing on render start**

Wherever the sidebar render function begins (top of `draw_sidebar` or similar), clear `state.divider_hits` before iterating, so stale hits from a previous frame don't linger.

If render does not currently take `&mut AppState`, the caller (in `app/render.rs` or `app/mod.rs`) should clear before invoking the renderer and pass `&mut Vec<DividerHit>` for the renderer to fill.

- [ ] **Step 7.4: Run cargo check**

```
cargo check
```
Expected: passes.

- [ ] **Step 7.5: Eyeball the divider**

```
cargo build --release && ./target/release/deck
```
Open a remote host divider should now show ` server-1 ────────── […]` at the right.

- [ ] **Step 7.6: Commit**

```
git add src/ui/sidebar.rs src/app/render.rs src/app/mod.rs
git commit -m "Render [...] button on remote-host dividers + record hit rect"
```

---

## Task 8: Action enum + reducer arms for `Pf*`

**Files:**
- Modify: `src/app/action/mod.rs` — add new `Action` variants
- Modify: `src/app/action/reduce.rs` — reducer arms

- [ ] **Step 8.1: Extend the `Action` enum**

Insert at the bottom of the variant list (before `None`):

```rust
    // Port-forward overlay (per-host)
    OpenHostDividerMenu { host: String, x: u16, y: u16 },
    OpenPortForward(String),
    PfClose,
    PfFocusUp,
    PfFocusDown,
    PfDelete,
    PfAddOpen,
    PfAddCancel,
    PfAddSubmit,
    PfAddFieldNext,
    PfAddFieldPrev,
    PfAddModeLeft,
    PfAddModeRight,
    PfAddInput(char),
    PfAddBackspace,
    PfTaskResult {
        host: String,
        op: crate::app::port_forward_task::OpKind,
        ok: bool,
        message: String,
    },
```

- [ ] **Step 8.2: Reducer arms in `src/app/action/reduce.rs`**

Find the big `match action` block and add arms (sketch — adapt to existing field names):

```rust
Action::OpenHostDividerMenu { host, x, y } => {
    state.overlay.context_menu = Some(ContextMenu {
        kind: MenuKind::HostDivider {
            host: host.clone(),
            items: host_divider_menu_items(),
        },
        x,
        y,
        selected: 0,
    });
}

Action::OpenPortForward(host) => {
    state.overlay.context_menu = None;
    state.overlay.port_forward = Some(PortForwardOverlay {
        host,
        selected: 0,
        add_form: None,
        status: None,
    });
}

Action::PfClose => {
    state.overlay.port_forward = None;
}

Action::PfFocusUp => {
    if let Some(o) = state.overlay.port_forward.as_mut() {
        o.selected = o.selected.saturating_sub(1);
    }
}
Action::PfFocusDown => {
    if let Some(o) = state.overlay.port_forward.as_mut() {
        let host = o.host.clone();
        let len = forwards_len(&state, &host);
        if o.selected + 1 < len {
            o.selected += 1;
        }
    }
}

Action::PfAddOpen => {
    if let Some(o) = state.overlay.port_forward.as_mut() {
        o.add_form = Some(PfAddForm::default_for(ForwardMode::Local));
        o.status = None;
    }
}
Action::PfAddCancel => {
    if let Some(o) = state.overlay.port_forward.as_mut() {
        o.add_form = None;
    }
}
Action::PfAddFieldNext => {
    if let Some(o) = state.overlay.port_forward.as_mut() {
        if let Some(f) = o.add_form.as_mut() {
            f.focus = next_field(f.focus, f.mode);
        }
    }
}
Action::PfAddFieldPrev => {
    if let Some(o) = state.overlay.port_forward.as_mut() {
        if let Some(f) = o.add_form.as_mut() {
            f.focus = prev_field(f.focus, f.mode);
        }
    }
}
Action::PfAddModeLeft => set_mode(&mut state.overlay.port_forward, -1),
Action::PfAddModeRight => set_mode(&mut state.overlay.port_forward, 1),
Action::PfAddInput(c) => {
    if let Some(o) = state.overlay.port_forward.as_mut() {
        if let Some(f) = o.add_form.as_mut() {
            push_char(f, c);
        }
    }
}
Action::PfAddBackspace => {
    if let Some(o) = state.overlay.port_forward.as_mut() {
        if let Some(f) = o.add_form.as_mut() {
            pop_char(f);
        }
    }
}

// These two stay no-ops here; the side effect that actually contacts
// the worker is dispatched in `dispatch.rs`. Reducer just clears state.
Action::PfAddSubmit | Action::PfDelete => {}

Action::PfTaskResult { host, op, ok, message } => {
    fx.merge(apply_pf_task_result(state, &host, &op, ok, &message));
}
```

Add the helpers at the bottom of the file:

```rust
fn forwards_len(state: &AppState, host: &str) -> usize {
    // Look up the host's persisted forwards in the in-memory copy
    // (Config is not held on AppState yet — see Task 9 for the
    // store; here, mirror via state.config_remotes added there).
    state
        .config_remotes
        .iter()
        .find(|r| r.host == host)
        .map(|r| r.forwards.len())
        .unwrap_or(0)
}

fn next_field(f: PfField, mode: ForwardMode) -> PfField {
    let order: &[PfField] = match mode {
        ForwardMode::Dynamic => &[PfField::Mode, PfField::BindAddr, PfField::ListenPort],
        _ => &[
            PfField::Mode,
            PfField::BindAddr,
            PfField::ListenPort,
            PfField::TargetHost,
            PfField::TargetPort,
        ],
    };
    let i = order.iter().position(|x| *x == f).unwrap_or(0);
    order[(i + 1) % order.len()]
}
fn prev_field(f: PfField, mode: ForwardMode) -> PfField {
    let order: &[PfField] = match mode {
        ForwardMode::Dynamic => &[PfField::Mode, PfField::BindAddr, PfField::ListenPort],
        _ => &[
            PfField::Mode,
            PfField::BindAddr,
            PfField::ListenPort,
            PfField::TargetHost,
            PfField::TargetPort,
        ],
    };
    let i = order.iter().position(|x| *x == f).unwrap_or(0);
    order[(i + order.len() - 1) % order.len()]
}
fn set_mode(o: &mut Option<PortForwardOverlay>, delta: i32) {
    if let Some(o) = o.as_mut() {
        if let Some(f) = o.add_form.as_mut() {
            let modes = [ForwardMode::Local, ForwardMode::Remote, ForwardMode::Dynamic];
            let i = modes.iter().position(|m| *m == f.mode).unwrap_or(0) as i32;
            let n = modes.len() as i32;
            let j = ((i + delta) % n + n) % n;
            f.mode = modes[j as usize];
            // Snap focus to a valid field if we just dropped Target fields.
            if matches!(f.mode, ForwardMode::Dynamic)
                && matches!(f.focus, PfField::TargetHost | PfField::TargetPort)
            {
                f.focus = PfField::ListenPort;
            }
        }
    }
}
fn push_char(f: &mut PfAddForm, c: char) {
    match f.focus {
        PfField::Mode => {}
        PfField::BindAddr => f.bind_addr.push(c),
        PfField::ListenPort => f.listen_port.push(c),
        PfField::TargetHost => f.target_host.push(c),
        PfField::TargetPort => f.target_port.push(c),
    }
}
fn pop_char(f: &mut PfAddForm) {
    let s = match f.focus {
        PfField::Mode => return,
        PfField::BindAddr => &mut f.bind_addr,
        PfField::ListenPort => &mut f.listen_port,
        PfField::TargetHost => &mut f.target_host,
        PfField::TargetPort => &mut f.target_port,
    };
    s.pop();
}

/// Finalize an in-flight `AddForward`. On success: append spec to
/// `config_remotes`, save config, close the add form. On failure:
/// keep the form open, clear `submitting`, set status to the error.
/// This is the *lazy persist* path — config is written only when the
/// worker confirms the forward actually took. Returns a SideEffect
/// the reducer caller will merge (so dispatch can persist).
fn apply_pf_task_result(
    state: &mut AppState,
    host: &str,
    op: &crate::app::port_forward_task::OpKind,
    ok: bool,
    message: &str,
) -> SideEffect {
    use crate::app::port_forward_task::OpKind;
    let mut fx = SideEffect::default();
    let Some(overlay) = state.overlay.port_forward.as_mut() else { return fx; };
    if overlay.host != host {
        return fx;
    }
    match op {
        OpKind::Forward(_, spec) => {
            if ok {
                if let Some(r) = state.config_remotes.iter_mut().find(|r| r.host == host) {
                    if !r.forwards.contains(spec) {
                        r.forwards.push(spec.clone());
                    }
                }
                overlay.add_form = None;
                overlay.status = Some("forward applied".into());
                fx.save_config = true;
            } else if let Some(f) = overlay.add_form.as_mut() {
                f.submitting = false;
                overlay.status = Some(format!("error: {}", message));
            } else {
                overlay.status = Some(format!("error: {}", message));
            }
        }
        OpKind::Cancel(_, _) => {
            overlay.status = Some(if ok {
                "forward cancelled".into()
            } else {
                format!("warn: cancel failed ({})", message)
            });
        }
        OpKind::Master(_) => {
            if !ok {
                overlay.status = Some(format!("master: {}", message));
            }
        }
        OpKind::Exit(_) => {
            if !ok {
                overlay.status = Some(format!("exit: {}", message));
            }
        }
    }
    fx
}
```

`state.config_remotes` is a new mirror added next step.

- [ ] **Step 8.3: Mirror `Config.remotes` onto `AppState`**

Add to `AppState`:

```rust
    /// Mirror of `Config.remotes` so reducers can read per-host forwards
    /// without round-tripping through dispatch. Kept in sync by startup
    /// and `reload_config`.
    pub config_remotes: Vec<crate::config::RemoteConfig>,
```

Initialize in `AppState::new`:

```rust
        config_remotes: Vec::new(),
```

Populate at app construction (in `App::new` or wherever the initial config is built) by assigning `state.config_remotes = cfg.remotes.clone();`. Also assign on reload (`reload_config`).

- [ ] **Step 8.4: Compile-check**

```
cargo check
```

- [ ] **Step 8.5: Reducer unit tests**

Append to `tests/unit/app/action/reduce.rs`:

```rust
#[test]
fn open_host_divider_menu_uses_host_kind() {
    let mut state = test_state();
    crate::action::apply_action(
        &mut state,
        Action::OpenHostDividerMenu { host: "h1".into(), x: 10, y: 5 },
    );
    let menu = state.overlay.context_menu.expect("menu opened");
    match menu.kind {
        MenuKind::HostDivider { host, .. } => assert_eq!(host, "h1"),
        _ => panic!("expected HostDivider"),
    }
}

#[test]
fn open_port_forward_clears_menu_and_opens_overlay() {
    let mut state = test_state();
    crate::action::apply_action(&mut state, Action::OpenPortForward("h1".into()));
    assert!(state.overlay.context_menu.is_none());
    let o = state.overlay.port_forward.as_ref().expect("overlay open");
    assert_eq!(o.host, "h1");
    assert_eq!(o.selected, 0);
}

#[test]
fn pf_add_open_creates_default_form() {
    let mut state = test_state();
    state.overlay.port_forward = Some(PortForwardOverlay {
        host: "h".into(), selected: 0, add_form: None, status: None,
    });
    crate::action::apply_action(&mut state, Action::PfAddOpen);
    let o = state.overlay.port_forward.as_ref().unwrap();
    let f = o.add_form.as_ref().unwrap();
    assert_eq!(f.mode, ForwardMode::Local);
    assert_eq!(f.focus, PfField::ListenPort);
}
```

`test_state()` likely already exists in this file; reuse it.

- [ ] **Step 8.6: Run tests**

```
cargo test --lib -- open_host_divider open_port_forward pf_add_open
```
Expected: 3 pass.

- [ ] **Step 8.7: Commit**

```
git add src/app/action/mod.rs src/app/action/reduce.rs src/model/state.rs tests/unit/app/action/reduce.rs
git commit -m "Add Pf* actions and reducer arms for port-forward overlay"
```

---

## Task 9: Worker channel + dispatch routing + save_config update

**Files:**
- Modify: `src/app/mod.rs` — store `Sender<Op>` on `App`, spawn worker on construct
- Modify: `src/app/dispatch.rs` — translate `PfAddSubmit` / `PfDelete` / overlay-close-with-pending into worker commands
- Modify: `src/app/update.rs` — change `save_config()` to use `state.config_remotes`

> **Heads-up:** the existing `App::save_config()` (in `src/app/update.rs:12`) explicitly re-reads remotes from disk on save, on the assumption that remotes are only mutated by the `deck remote` CLI. With this feature, `forwards` is UI-mutated, so save must write `state.config_remotes`. Hot-reload still covers CLI-side changes to remotes themselves.

- [ ] **Step 9.1: Spawn worker in `App::new`**

Add a field on `App`:

```rust
    port_forward_tx: std::sync::mpsc::Sender<crate::app::port_forward_task::Op>,
```

In `App::new`, near the other init steps:

```rust
    let (pf_result_tx, pf_result_rx) = std::sync::mpsc::channel();
    let port_forward_tx = crate::app::port_forward_task::spawn(pf_result_tx);
```

Store the `Receiver<OpResult>` on `App` too:

```rust
    port_forward_rx: std::sync::mpsc::Receiver<crate::app::port_forward_task::OpResult>,
```

- [ ] **Step 9.2: Drain results in the main loop**

In the event loop (`App::run` or whichever function polls events on a tick), after the existing config-mtime check (around line 459), add:

```rust
    while let Ok(r) = self.port_forward_rx.try_recv() {
        let host = match &r.kind {
            crate::app::port_forward_task::OpKind::Master(h)
            | crate::app::port_forward_task::OpKind::Forward(h, _)
            | crate::app::port_forward_task::OpKind::Cancel(h, _)
            | crate::app::port_forward_task::OpKind::Exit(h) => h.clone(),
        };
        self.dispatch(Action::PfTaskResult {
            host,
            op: r.kind,
            ok: r.ok,
            message: r.message,
        });
    }
```

- [ ] **Step 9.3: Translate `PfAddSubmit` / `PfDelete` in `dispatch.rs`**

Add to the explicit arm list in `dispatch.rs` (alongside `Action::ReloadConfig`):

```rust
Action::PfAddSubmit => {
    self.pf_add_submit();
    false
}
Action::PfDelete => {
    self.pf_delete_selected();
    false
}
```

Implement the two methods:

```rust
/// Validate the add form. On validate-failure: set status, form stays
/// open, no worker call. On validate-success: send `AddForward` to
/// worker, mark form `submitting=true`, set status to "applying...".
/// **Lazy persist:** config is NOT modified here; the reducer for
/// `PfTaskResult` writes it on worker success.
fn pf_add_submit(&mut self) {
    let Some(overlay) = self.state.overlay.port_forward.as_mut() else { return; };
    let Some(form) = overlay.add_form.as_mut() else { return; };
    if form.submitting {
        return; // ignore double-Enter
    }
    let spec = match form.validate() {
        Ok(s) => s,
        Err(e) => {
            overlay.status = Some(format!("err: {}", e.message()));
            return;
        }
    };
    let host = overlay.host.clone();
    form.submitting = true;
    overlay.status = Some("applying...".into());
    let _ = self.port_forward_tx.send(
        crate::app::port_forward_task::Op::AddForward { host, spec },
    );
}

/// Cancel-then-remove. Spec semantics: remove from config regardless
/// of worker outcome (avoid ghost entries). Save via the side-effect
/// path so the existing `save_config` plumbing handles it.
fn pf_delete_selected(&mut self) {
    let (host, spec) = {
        let Some(overlay) = self.state.overlay.port_forward.as_ref() else { return; };
        let host = overlay.host.clone();
        let idx = overlay.selected;
        let Some(spec) = self
            .state
            .config_remotes
            .iter()
            .find(|r| r.host == host)
            .and_then(|r| r.forwards.get(idx))
            .cloned()
        else {
            return;
        };
        (host, spec)
    };

    persist_forward(&mut self.state.config_remotes, &host, spec.clone(), /*add=*/ false);
    self.save_config();
    if let Some(overlay) = self.state.overlay.port_forward.as_mut() {
        overlay.status = Some("cancelling...".into());
        // Selected index may now be out of range — clamp.
        let len = self
            .state
            .config_remotes
            .iter()
            .find(|r| r.host == host)
            .map(|r| r.forwards.len())
            .unwrap_or(0);
        if overlay.selected >= len && len > 0 {
            overlay.selected = len - 1;
        }
    }
    let _ = self.port_forward_tx.send(
        crate::app::port_forward_task::Op::CancelForward { host, spec },
    );
}
```

And a helper:

```rust
fn persist_forward(
    remotes: &mut Vec<crate::config::RemoteConfig>,
    host: &str,
    spec: crate::config::ForwardSpec,
    add: bool,
) {
    if let Some(r) = remotes.iter_mut().find(|r| r.host == host) {
        if add {
            r.forwards.push(spec);
        } else {
            r.forwards.retain(|s| *s != spec);
        }
    }
}
```

`snapshot_config()` — if there's no existing helper, write one that re-assembles `Config` from `self.state` (analogous to whatever `dispatch.rs` does when it persists settings page changes; grep for `Config { ` to find the pattern).

- [ ] **Step 9.4: Update `save_config` to use `state.config_remotes`**

In `src/app/update.rs:12`, replace the `Config::load().remotes` line:

```rust
pub(super) fn save_config(&self) {
    Config {
        theme: THEMES[self.state.theme_index].name.to_string(),
        layout: self.state.layout_mode,
        show_borders: self.state.show_borders,
        sidebar_width: self.state.sidebar_width,
        sidebar_height: self.state.sidebar_height,
        view_mode: self.state.view_mode,
        exclude_patterns: self.state.exclude_patterns.clone(),
        plugins: self.state.plugins.clone(),
        keybindings: self.raw_keybindings.clone(),
        update_check: self.state.update_check_mode,
        remotes: self.state.config_remotes.clone(),
    }
    .save();
}
```

The previous "re-read from disk to avoid clobbering CLI" was correct when UI never touched remotes. Now that the UI mutates `forwards`, the in-memory `config_remotes` is the source of truth. Hot-reload (Task 14) still folds CLI-side host additions/removals back into `config_remotes`, so the CLI path remains safe.

- [ ] **Step 9.5: Compile-check**

```
cargo check
```

- [ ] **Step 9.6: Commit**

```
git add src/app/mod.rs src/app/dispatch.rs src/app/update.rs
git commit -m "Wire port-forward worker channel; save remotes from state"
```

---

## Task 10: Startup bootstrap

**Files:**
- Modify: `src/app/mod.rs` (or `src/main.rs` — whichever owns the construction sequence)

- [ ] **Step 10.1: Send `Bootstrap` once at startup**

After `port_forward_tx` is created and `cfg.remotes` is known, but before the main loop starts:

```rust
let hosts: Vec<(String, Vec<crate::config::ForwardSpec>)> = cfg
    .remotes
    .iter()
    .filter(|r| !r.forwards.is_empty())
    .map(|r| (r.host.clone(), r.forwards.clone()))
    .collect();
if !hosts.is_empty() {
    let _ = port_forward_tx.send(crate::app::port_forward_task::Op::Bootstrap { hosts });
}
```

- [ ] **Step 10.2: Manual smoke test**

Pick a real SSH-reachable host. Edit `~/.config/deck/config.json` to add a forward:

```json
"remotes": [{
  "host": "your-host",
  "forwards": [{ "mode": "local", "listen_port": 18080, "target_host": "localhost", "target_port": 80 }]
}]
```

Run `./target/release/deck`. From another terminal, run `nc -vz localhost 18080` or `curl localhost:18080`. Expect connection to succeed if the remote's port 80 is up.

- [ ] **Step 10.3: Commit**

```
git add src/app/mod.rs src/main.rs
git commit -m "Apply configured port forwards at deck startup"
```

---

## Task 11: PortForward overlay rendering

**Files:**
- Create: `src/ui/overlays/mod.rs` (if not present) — re-export
- Create: `src/ui/overlays/port_forward.rs`
- Modify: `src/ui/mod.rs` and the renderer entry point to call it

Note: the existing `src/ui/overlays.rs` is a flat module. Convert it to a folder by:
1. Renaming `src/ui/overlays.rs` → `src/ui/overlays/mod.rs`.
2. Adding `pub mod port_forward;` to `mod.rs`.

If the project prefers flat modules, place this file at `src/ui/overlays_port_forward.rs` and adjust `mod` declarations in `src/ui/mod.rs`. Adapt to whichever the maintainer prefers — grep for `pub mod overlays;` in `src/ui/mod.rs` to see current state.

- [ ] **Step 11.1: Implement `src/ui/overlays/port_forward.rs`**

```rust
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, BorderType, Clear, Paragraph, Widget};

use crate::config::{ForwardMode, ForwardSpec, RemoteConfig};
use crate::state::{PfAddForm, PfField, PortForwardOverlay};
use crate::theme::Theme;

const OVERLAY_WIDTH: u16 = 64;

pub fn draw_port_forward(
    buf: &mut Buffer,
    area: Rect,
    overlay: &PortForwardOverlay,
    remotes: &[RemoteConfig],
    theme: &Theme,
) {
    let forwards: Vec<ForwardSpec> = remotes
        .iter()
        .find(|r| r.host == overlay.host)
        .map(|r| r.forwards.clone())
        .unwrap_or_default();

    let body_height = if overlay.add_form.is_some() {
        10
    } else {
        (forwards.len().max(1) as u16) + 4
    };
    let total_height = body_height + 4; // top + status + bottom + borders
    let modal = centered_rect(area, OVERLAY_WIDTH, total_height);

    Clear.render(modal, buf);

    let title = match &overlay.add_form {
        Some(_) => format!("Port Forward — {}  ▸ add", overlay.host),
        None => format!("Port Forward — {}", overlay.host),
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .style(Style::default().bg(theme.surface).fg(theme.text));
    let inner = block.inner(modal);
    block.render(modal, buf);

    match &overlay.add_form {
        None => draw_list(buf, inner, &forwards, overlay, theme),
        Some(form) => draw_form(buf, inner, form, theme),
    }
}

fn draw_list(
    buf: &mut Buffer,
    area: Rect,
    forwards: &[ForwardSpec],
    overlay: &PortForwardOverlay,
    theme: &Theme,
) {
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::raw(""));
    if forwards.is_empty() {
        lines.push(Line::styled(
            "  (no forwards configured — press a to add)",
            Style::default().fg(theme.muted),
        ));
    } else {
        for (i, f) in forwards.iter().enumerate() {
            let marker = if i == overlay.selected { ">" } else { " " };
            let row = format!("  {} {}", marker, format_forward(f));
            let style = if i == overlay.selected {
                Style::default().fg(theme.accent).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.text)
            };
            lines.push(Line::styled(row, style));
        }
    }
    lines.push(Line::raw(""));
    if let Some(s) = &overlay.status {
        lines.push(Line::styled(
            format!("  status: {}", s),
            Style::default().fg(theme.muted),
        ));
    } else {
        lines.push(Line::raw(""));
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "  [a] add   [d] delete   [esc] close",
        Style::default().fg(theme.subtle),
    ));

    Paragraph::new(lines).render(area, buf);
}

fn draw_form(buf: &mut Buffer, area: Rect, form: &PfAddForm, theme: &Theme) {
    let mode_text = |m: ForwardMode, label: &str| -> Span {
        let marker = if form.mode == m { "(•)" } else { "( )" };
        Span::styled(
            format!("{} {}  ", marker, label),
            Style::default().fg(if form.focus == PfField::Mode && form.mode == m {
                theme.accent
            } else {
                theme.text
            }),
        )
    };

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::raw(""));
    lines.push(Line::from(vec![
        Span::raw("  mode:        "),
        mode_text(ForwardMode::Local, "local"),
        mode_text(ForwardMode::Remote, "remote"),
        mode_text(ForwardMode::Dynamic, "dynamic"),
    ]));
    lines.push(Line::raw(""));
    lines.push(field_line(theme, form, PfField::BindAddr,   "  bind addr:   ", &form.bind_addr,   true));
    lines.push(field_line(theme, form, PfField::ListenPort, "  listen port: ", &form.listen_port, true));
    let target_active = !matches!(form.mode, ForwardMode::Dynamic);
    lines.push(field_line(theme, form, PfField::TargetHost, "  target host: ", &form.target_host, target_active));
    lines.push(field_line(theme, form, PfField::TargetPort, "  target port: ", &form.target_port, target_active));
    lines.push(Line::raw(""));
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "  [tab] next   [enter] save   [esc] cancel",
        Style::default().fg(theme.subtle),
    ));

    Paragraph::new(lines).render(area, buf);
}

fn field_line<'a>(
    theme: &Theme,
    form: &PfAddForm,
    field: PfField,
    label: &'a str,
    value: &str,
    enabled: bool,
) -> Line<'a> {
    let focused = form.focus == field && enabled;
    let label_style = if focused {
        Style::default().fg(theme.accent).add_modifier(Modifier::BOLD)
    } else if enabled {
        Style::default().fg(theme.text)
    } else {
        Style::default().fg(theme.dim)
    };
    let cursor = if focused { "█" } else { "" };
    Line::from(vec![
        Span::styled(label.to_string(), label_style),
        Span::styled(format!("[{}{}]", value, cursor), label_style),
    ])
}

fn format_forward(f: &ForwardSpec) -> String {
    let bind = f.bind_addr.as_deref().map(|b| format!("{}:", b)).unwrap_or_default();
    match f.mode {
        ForwardMode::Local => format!(
            "-L {}{}  → {}:{}",
            bind,
            f.listen_port,
            f.target_host.as_deref().unwrap_or(""),
            f.target_port.unwrap_or(0)
        ),
        ForwardMode::Remote => format!(
            "-R {}{}  → {}:{}",
            bind,
            f.listen_port,
            f.target_host.as_deref().unwrap_or(""),
            f.target_port.unwrap_or(0)
        ),
        ForwardMode::Dynamic => format!("-D {}{}", bind, f.listen_port),
    }
}

fn centered_rect(area: Rect, w: u16, h: u16) -> Rect {
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    Rect { x, y, width: w.min(area.width), height: h.min(area.height) }
}
```

- [ ] **Step 11.2: Call from main renderer**

Find where other overlays are drawn (e.g., the block in `app/render.rs` that calls `draw_help`, `draw_kill_confirm`, etc.). Add:

```rust
if let Some(overlay) = state.overlay.port_forward.as_ref() {
    crate::ui::overlays::port_forward::draw_port_forward(
        frame.buffer_mut(),
        frame.size(),
        overlay,
        &state.config_remotes,
        &theme,
    );
}
```

- [ ] **Step 11.3: Compile-check**

```
cargo check
```

- [ ] **Step 11.4: Commit**

```
git add src/ui/overlays src/ui/mod.rs src/app/render.rs
git commit -m "Render port-forward list + add form overlays"
```

---

## Task 12: Mouse hit-test for divider button

**Files:**
- Modify: `src/app/action/mouse.rs`

- [ ] **Step 12.1: Add divider-hit check at the top of left-click handling**

Find the left-click arm (around mouse.rs:90). Before falling through to `focus_at_row()`, check `state.divider_hits`:

```rust
for hit in &state.divider_hits {
    if mouse.column >= hit.rect.x
        && mouse.column < hit.rect.x + hit.rect.width
        && mouse.row >= hit.rect.y
        && mouse.row < hit.rect.y + hit.rect.height
    {
        return Action::OpenHostDividerMenu {
            host: hit.host.clone(),
            x: hit.rect.x,
            y: hit.rect.y + 1, // open just below the button
        };
    }
}
```

- [ ] **Step 12.2: Make MenuClickItem aware of HostDivider menu**

Find the existing arm that dispatches `MenuClickItem(idx)` to the right next action based on `MenuKind`. Add a branch:

```rust
MenuKind::HostDivider { host, .. } => {
    // Only one item today.
    if idx == 0 {
        return Action::OpenPortForward(host.clone());
    }
    Action::MenuDismiss
}
```

Also extend `Action::MenuConfirm` handling the same way for keyboard menu use (if the existing code dispatches by `MenuKind` on keyboard confirm). Grep for `MenuConfirm` to find it.

- [ ] **Step 12.3: Compile-check**

```
cargo check
```

- [ ] **Step 12.4: Commit**

```
git add src/app/action/mouse.rs src/app/action/reduce.rs
git commit -m "Wire mouse click on [...] divider button to host-divider menu"
```

---

## Task 13: Keyboard handling — overlay-active routing + `f` shortcut

**Files:**
- Modify: `src/app/action/keyboard.rs`

- [ ] **Step 13.1: Find the overlay routing in `key_to_action`**

Look at how existing overlays (e.g., `renaming`, `exclude_editor`) intercept keys. The pattern is typically: at the top of `key_to_action`, check each overlay field and route keys accordingly before falling through to global bindings.

Add an early branch:

```rust
if let Some(overlay) = state.overlay.port_forward.as_ref() {
    return pf_key(key, overlay);
}
```

Implement `pf_key`:

```rust
fn pf_key(key: KeyEvent, overlay: &PortForwardOverlay) -> Action {
    use KeyCode::*;
    // Add form: char input mode
    if overlay.add_form.is_some() {
        match key.code {
            Esc => Action::PfAddCancel,
            Enter => Action::PfAddSubmit,
            Tab => Action::PfAddFieldNext,
            BackTab => Action::PfAddFieldPrev,
            Left => {
                // Only meaningful on Mode field; harmless elsewhere.
                if matches!(overlay.add_form.as_ref().unwrap().focus, PfField::Mode) {
                    Action::PfAddModeLeft
                } else {
                    Action::None
                }
            }
            Right => {
                if matches!(overlay.add_form.as_ref().unwrap().focus, PfField::Mode) {
                    Action::PfAddModeRight
                } else {
                    Action::None
                }
            }
            Backspace => Action::PfAddBackspace,
            Char(c) => Action::PfAddInput(c),
            _ => Action::None,
        }
    } else {
        match key.code {
            Esc => Action::PfClose,
            Char('a') => Action::PfAddOpen,
            Char('d') => Action::PfDelete,
            Up | Char('k') => Action::PfFocusUp,
            Down | Char('j') => Action::PfFocusDown,
            _ => Action::None,
        }
    }
}
```

- [ ] **Step 13.2: Add `f` shortcut when remote session is focused**

In the global-binding section (below the overlay routing), where similar single-char shortcuts live, add:

```rust
KeyCode::Char('f') => {
    if let Some(target) = state.focus_target() {
        if let Some(SessionTargetRef::Remote(r)) = state.session_target(target) {
            return Action::OpenPortForward(r.host.clone());
        }
    }
    Action::None
}
```

If keybindings are configurable (they appear to be via `Keybindings`), check whether `f` collides with an existing binding by searching `src/model/keybindings.rs` for `'f'` or `"f"`. If it does, use `F` instead and call it out in the commit.

- [ ] **Step 13.3: Compile-check + manual run**

```
cargo build --release && ./target/release/deck
```

Manually: click `[…]` on a remote divider → menu pops up → click Port Forward → overlay appears. Press `a` → form appears. Type values, press Tab, Enter. Press `esc` to close.

- [ ] **Step 13.4: Commit**

```
git add src/app/action/keyboard.rs
git commit -m "Route keys to port-forward overlay and bind 'f' shortcut"
```

---

## Task 14: Hot-reload diff integration

**Files:**
- Modify: `src/app/dispatch.rs::reload_config`

- [ ] **Step 14.1: Diff forwards in `reload_config`**

After parsing `cfg` and before assigning fields, compute the per-host diff against the current `state.config_remotes`:

```rust
let old_remotes = std::mem::take(&mut self.state.config_remotes);
let new_remotes = cfg.remotes.clone();

// Hosts only in old → stop master.
for old in &old_remotes {
    if !new_remotes.iter().any(|n| n.host == old.host) {
        let _ = self
            .port_forward_tx
            .send(crate::app::port_forward_task::Op::StopHost { host: old.host.clone() });
        continue;
    }
}

// Per-host diff for hosts present in both / new only.
for n in &new_remotes {
    let empty = Vec::new();
    let old_fwds: &[crate::config::ForwardSpec] = old_remotes
        .iter()
        .find(|o| o.host == n.host)
        .map(|o| o.forwards.as_slice())
        .unwrap_or(&empty);
    for op in crate::config::diff_forwards(old_fwds, &n.forwards) {
        let msg = match op {
            crate::config::ForwardOp::Add(spec) => crate::app::port_forward_task::Op::AddForward {
                host: n.host.clone(), spec,
            },
            crate::config::ForwardOp::Cancel(spec) => {
                crate::app::port_forward_task::Op::CancelForward { host: n.host.clone(), spec }
            }
        };
        let _ = self.port_forward_tx.send(msg);
    }
}
self.state.config_remotes = new_remotes;
```

- [ ] **Step 14.2: Compile-check + manual test**

```
cargo build --release && ./target/release/deck &
# in another terminal: edit ~/.config/deck/config.json, change forwards
```

Verify status messages flicker through the overlay if open, and that `lsof -iTCP:<port>` shows the forward appearing/disappearing.

- [ ] **Step 14.3: Commit**

```
git add src/app/dispatch.rs
git commit -m "Apply forward diff on config hot-reload"
```

---

## Task 15: Sweep — verification + sanity

- [ ] **Step 15.1: Run all tests**

```
cargo test
```
Expected: all pass.

- [ ] **Step 15.2: Run clippy**

```
cargo clippy --all-targets -- -D warnings
```
Expected: clean. Fix any warnings inline.

- [ ] **Step 15.3: Manual checklist**

- [ ] Mouse click on `[…]` opens menu near button
- [ ] Menu's `Port Forward` opens the overlay
- [ ] `f` while on a remote session opens the overlay directly
- [ ] `esc` closes overlay
- [ ] `a` opens add form; mode picker via ←/→ on Mode field
- [ ] Tab cycles fields; for Dynamic, Target fields are skipped
- [ ] Invalid input shows `err:` line, no worker call
- [ ] Valid input applies forward, status updates to "forward applied"
- [ ] Selected forward + `d` removes it from list and stops it
- [ ] `config.json` reflects current forwards
- [ ] Restart deck — forwards re-apply at startup (verify with `lsof -iTCP:<port>`)
- [ ] Edit `config.json` externally — diff is applied without restart
- [ ] Wrong host (unreachable) — overlay status shows error, deck stays usable

- [ ] **Step 15.4: Tag and merge**

```
git push -u origin feature/port-forward
gh pr create --title "Port forward feature: per-host SSH tunnels from sidebar" --body "$(cat <<'EOF'
## Summary
- Adds `[…]` button on remote-host dividers; opens menu → port-forward overlay.
- Supports `-L`, `-R`, `-D` modes with list + add subform.
- Persists to `config.json`; applies eagerly at startup; immediate on UI edits.
- Hot-reload diffs old vs new forwards and applies the delta.

## Test plan
- [ ] `cargo test`
- [ ] Manual: see plan §15.3 checklist

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

(User asked earlier to skip PRs and merge directly; if that preference still holds, fast-forward merge instead.)

---

## Spec coverage check

| Spec section | Implemented in |
|---|---|
| `ForwardSpec`/`ForwardMode`/`RemoteConfig.forwards` + `to_ssh_flag` | Task 1 |
| Validation invariants (port range, missing target, Dynamic clears target) | Task 4 |
| SSH flag projection table | Task 1 |
| Runtime state (`PortForwardOverlay`, `PfAddForm`, `PfField`) | Task 4, 5 |
| `MenuKind::HostDivider` + `HOST_DIVIDER_MENU_ITEMS` | Task 5, 8 |
| `DividerHit` | Task 5, 7 |
| Divider button rendering | Task 7 |
| Trigger paths (mouse / `f` / menu) | Task 12, 13 |
| Main overlay (list) | Task 11 |
| Add subform | Task 11 |
| Startup eager bootstrap | Task 10 |
| Runtime add / cancel | Task 6, 9 |
| Worker (`ssh -fN`, `-O forward`, `-O cancel`, `-O exit`) | Task 2, 6 |
| `PfTaskResult` reducer | Task 8 |
| Hot-reload diff integration | Task 3, 14 |
| Error handling (port collision, unreachable, invalid form, ghost cancel) | Task 6 (master fail), Task 8 (status), Task 9 (validation msg) |
| Tests: command builder, worker ordering, form validate, config roundtrip, diff | Task 1, 2, 3, 4, 6 |

Open question 1 (shared ControlPath helper) resolved by `ssh_args_for_host` in `infra/port_forward.rs`. If `app/remote_spawn.rs` does not call this helper today, after the feature lands a separate cleanup PR can refactor `remote_spawn.rs` to use it too — outside this plan's scope but worth a follow-up task.

Open question 2 (`f` key conflict) resolved in Task 13.3.
