//! Port-forward model: the persisted forward rule (`ForwardSpec` /
//! `ForwardMode`), its config-diff (`ForwardOp` / `diff_forwards`), and the
//! add-forward overlay form state.

use ratatui_textarea::TextArea;
use serde::{Deserialize, Serialize};

use crate::new_session::{make_textarea, textarea_line};
use crate::system::ForwardEndpointKind;

// --- Persisted forward rule ---

/// One SSH port-forward rule. Maps to a single `-L`, `-R`, or `-D` flag.
/// Persisted as part of `RemoteConfig` in the YAML config.
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum ForwardMode {
    Local,
    Remote,
    Dynamic,
}

impl ForwardMode {
    /// The ssh flag this mode maps to.
    pub fn flag(self) -> &'static str {
        match self {
            ForwardMode::Local => "-L",
            ForwardMode::Remote => "-R",
            ForwardMode::Dynamic => "-D",
        }
    }
}

impl ForwardSpec {
    /// The pair `("-L" | "-R" | "-D", "<bind?>:listen:<target_host:target_port?>")`
    /// suitable for `Command::arg(flag).arg(value)`. Use this when you need the
    /// flag and value as separate arg slots (e.g., `ssh -O forward -L 8080:host:80`).
    pub fn ssh_flag_and_value(&self) -> (&'static str, String) {
        let bind_prefix = match &self.bind_addr {
            Some(b) => format!("{}:", b),
            None => String::new(),
        };
        let value = match self.mode {
            ForwardMode::Dynamic => format!("{}{}", bind_prefix, self.listen_port),
            ForwardMode::Local | ForwardMode::Remote => {
                let th = self.target_host.as_deref().unwrap_or("");
                let tp = self.target_port.unwrap_or(0);
                format!("{}{}:{}:{}", bind_prefix, self.listen_port, th, tp)
            }
        };
        (self.mode.flag(), value)
    }

    /// Render this rule as the corresponding `ssh -L/-R/-D` argument
    /// string. Test-only helper over `ssh_flag_and_value`.
    #[cfg(test)]
    pub fn to_ssh_flag(&self) -> String {
        let (flag, value) = self.ssh_flag_and_value();
        format!("{} {}", flag, value)
    }

    /// Whether two forwards would claim the same local listener: same mode,
    /// bind address, and listen port. The target endpoint is irrelevant — ssh
    /// can't bind one port twice — so this is the identity used to reject a
    /// duplicate before handing it to ssh.
    pub fn same_listen_identity(&self, other: &ForwardSpec) -> bool {
        self.mode == other.mode
            && self.bind_addr == other.bind_addr
            && self.listen_port == other.listen_port
    }
}

/// Difference between two `Vec<ForwardSpec>` slices: which to add, which
/// to cancel. Order-insensitive; equal specs (all fields) are the same.
/// Used by UI edits (single ops) and hot-reload (bulk).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForwardOp {
    Add(ForwardSpec),
    Cancel(ForwardSpec),
}

pub fn diff_forwards(old: &[ForwardSpec], new: &[ForwardSpec]) -> Vec<ForwardOp> {
    // Cancels first, then adds — callers rely on that order.
    old.iter()
        .filter(|o| !new.contains(o))
        .cloned()
        .map(ForwardOp::Cancel)
        .chain(
            new.iter()
                .filter(|n| !old.contains(n))
                .cloned()
                .map(ForwardOp::Add),
        )
        .collect()
}

// --- Port forward overlay ---

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PfField {
    Mode,
    BindAddr,
    ListenPort,
    TargetHost,
    TargetPort,
}

/// One input field, backed by `ratatui-textarea`. Each field carries
/// its own cursor and edit history; the keyboard dispatcher feeds key
/// events to whichever one is focused.
#[derive(Debug, Clone)]
pub struct PfAddForm {
    pub mode: ForwardMode,
    /// What this lane's forwards point at, from its own capabilities. Decides
    /// which modes the form offers and whether it asks for a target address at
    /// all — see [`ForwardEndpointKind`].
    pub endpoint: ForwardEndpointKind,
    pub focus: PfField,
    pub bind_addr: TextArea<'static>,
    pub listen_port: TextArea<'static>,
    pub target_host: TextArea<'static>,
    pub target_port: TextArea<'static>,
    /// True while a validated spec is in flight to the worker; the form stays
    /// rendered read-only until `PfTaskResult` clears or fails it. Lazy
    /// persist: config is written only when the worker reports success.
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
            PfFormError::ListenPortRange => "Listen port must be a number from 0 to 65535.",
            PfFormError::TargetPortRange => "Target port must be a number from 0 to 65535.",
            PfFormError::TargetHostRequired => "Target host is required for -L and -R forwards.",
        }
    }
}

impl PfAddForm {
    /// A fresh form for `lane_label`'s lane. On a lane that *is* the endpoint
    /// the target-host field is seeded with the lane's own name and never
    /// edited: it is there so the flow sketch reads as the user thinks of it
    /// ("to the container"), not as something to fill in.
    pub fn default_for(mode: ForwardMode, endpoint: ForwardEndpointKind, lane_label: &str) -> Self {
        let mode = match endpoint {
            ForwardEndpointKind::Explicit => mode,
            ForwardEndpointKind::Lane => ForwardMode::Local,
        };
        Self {
            mode,
            endpoint,
            focus: PfField::ListenPort,
            bind_addr: make_textarea("0.0.0.0"),
            listen_port: make_textarea(""),
            target_host: make_textarea(match endpoint {
                ForwardEndpointKind::Explicit => "127.0.0.1",
                ForwardEndpointKind::Lane => lane_label,
            }),
            target_port: make_textarea(""),
            submitting: false,
        }
    }

    /// The modes this form offers, in picker order.
    pub fn modes(&self) -> &'static [ForwardMode] {
        match self.endpoint {
            ForwardEndpointKind::Explicit => &[
                ForwardMode::Local,
                ForwardMode::Remote,
                ForwardMode::Dynamic,
            ],
            ForwardEndpointKind::Lane => &[ForwardMode::Local],
        }
    }

    /// Whether the user names the target address. False when the lane is the
    /// endpoint: the address is resolved at apply time, so a typed one would be
    /// wrong the first time the container restarted.
    pub fn asks_target_host(&self) -> bool {
        matches!(self.endpoint, ForwardEndpointKind::Explicit)
            && !matches!(self.mode, ForwardMode::Dynamic)
    }

    /// Read the current text of a field. Returns `""` for `Mode`.
    pub fn field_text(&self, field: PfField) -> &str {
        match field {
            PfField::Mode => "",
            PfField::BindAddr => textarea_line(&self.bind_addr),
            PfField::ListenPort => textarea_line(&self.listen_port),
            PfField::TargetHost => textarea_line(&self.target_host),
            PfField::TargetPort => textarea_line(&self.target_port),
        }
    }

    /// Mutable handle to the focused field's textarea. `None` for `Mode`.
    pub fn focused_textarea_mut(&mut self) -> Option<&mut TextArea<'static>> {
        match self.focus {
            PfField::Mode => None,
            PfField::BindAddr => Some(&mut self.bind_addr),
            PfField::ListenPort => Some(&mut self.listen_port),
            PfField::TargetHost => Some(&mut self.target_host),
            PfField::TargetPort => Some(&mut self.target_port),
        }
    }

    pub fn validate(&self) -> Result<ForwardSpec, PfFormError> {
        // A lane that is its own endpoint takes the port and nothing else: the
        // address is not the user's to give, so `target_host` stays `None` and
        // the worker fills it in from the lane each time the rule is applied.
        if matches!(self.endpoint, ForwardEndpointKind::Lane) {
            let listen_port: u16 = self
                .field_text(PfField::ListenPort)
                .trim()
                .parse()
                .map_err(|_| PfFormError::ListenPortRange)?;
            let target_port: u16 = self
                .field_text(PfField::TargetPort)
                .trim()
                .parse()
                .map_err(|_| PfFormError::TargetPortRange)?;
            let bind_raw = self.field_text(PfField::BindAddr).trim();
            return Ok(ForwardSpec {
                mode: ForwardMode::Local,
                bind_addr: (!bind_raw.is_empty()).then(|| bind_raw.to_string()),
                listen_port,
                target_host: None,
                target_port: Some(target_port),
            });
        }
        // Trim defensively even though input filtering already blocks
        // whitespace, so any value is persisted clean. `u16::parse` enforces
        // the 0..=65535 range; port 0 ("let kernel pick") is accepted.
        let listen_port: u16 = self
            .field_text(PfField::ListenPort)
            .trim()
            .parse()
            .map_err(|_| PfFormError::ListenPortRange)?;
        let bind_raw = self.field_text(PfField::BindAddr).trim();
        let bind_addr = (!bind_raw.is_empty()).then(|| bind_raw.to_string());

        match self.mode {
            ForwardMode::Dynamic => Ok(ForwardSpec {
                mode: ForwardMode::Dynamic,
                bind_addr,
                listen_port,
                target_host: None,
                target_port: None,
            }),
            ForwardMode::Local | ForwardMode::Remote => {
                let target_host = self.field_text(PfField::TargetHost).trim();
                if target_host.is_empty() {
                    return Err(PfFormError::TargetHostRequired);
                }
                let target_port: u16 = self
                    .field_text(PfField::TargetPort)
                    .trim()
                    .parse()
                    .map_err(|_| PfFormError::TargetPortRange)?;
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
    pub lane: crate::lane::LaneId,
    pub selected: usize,
    pub add_form: Option<PfAddForm>,
    pub status: Option<String>,
}

#[cfg(test)]
#[path = "../../../../tests/unit/model/forwards.rs"]
mod tests;
