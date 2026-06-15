//! Port-forward model: per-forward liveness (`ForwardHealth` / `ForwardKey`),
//! the divider badge rollup, and the add-forward overlay form state.

use ratatui_textarea::TextArea;

use crate::new_session::{make_textarea, textarea_line};

// --- Port-forward liveness types ---

/// Liveness of a single configured forward, refreshed each probe tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForwardHealth {
    /// Not yet probed this session, enumeration was unavailable, or the host
    /// is still connecting.
    Probing,
    /// `-L`/`-D`: a local listener is present on the listen port. `-R`: the
    /// host connection is up (the remote listener can't be confirmed locally,
    /// so it simply tracks reachability).
    Up,
    /// `-L`/`-D`: no local listener. `-R`: the host is unreachable.
    Down,
}

/// Stable identity of a configured forward, keying liveness across config
/// reloads and reorders. `mode` and `bind_addr` are included alongside the
/// listen port so an `-L` and `-R` sharing a port number don't collide.
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

// --- Divider badge rollup ---

/// Aggregate liveness of a host's configured forwards, shown as a colored
/// `[⇄N]` badge on the remote `@host` divider (left of `[⟳]`). `total` is the
/// forward count; `status` is the rollup color the renderer paints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ForwardBadge {
    pub total: usize,
    pub status: ForwardBadgeStatus,
}

/// Traffic-light state of a [`ForwardBadge`]: green = all up, red = all down,
/// orange = mixed, "probing" (neutral) = none confirmed either way yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForwardBadgeStatus {
    AllUp,
    AllDown,
    Mixed,
    Probing,
}

impl ForwardBadge {
    /// Roll a host's per-forward health up into a single badge. Returns `None`
    /// when the host has no configured forwards (no badge is drawn).
    pub fn rollup(healths: impl Iterator<Item = ForwardHealth>) -> Option<Self> {
        let (mut up, mut down, mut total) = (0usize, 0usize, 0usize);
        for h in healths {
            total += 1;
            match h {
                ForwardHealth::Up => up += 1,
                ForwardHealth::Down => down += 1,
                ForwardHealth::Probing => {}
            }
        }
        if total == 0 {
            return None;
        }
        let status = if up == total {
            ForwardBadgeStatus::AllUp
        } else if down == total {
            ForwardBadgeStatus::AllDown
        } else if up == 0 && down == 0 {
            ForwardBadgeStatus::Probing
        } else {
            ForwardBadgeStatus::Mixed
        };
        Some(Self { total, status })
    }
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
    pub mode: crate::config::ForwardMode,
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
    pub fn default_for(mode: crate::config::ForwardMode) -> Self {
        Self {
            mode,
            focus: PfField::ListenPort,
            bind_addr: make_textarea("0.0.0.0"),
            listen_port: make_textarea(""),
            target_host: make_textarea("127.0.0.1"),
            target_port: make_textarea(""),
            submitting: false,
        }
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

    pub fn validate(&self) -> Result<crate::config::ForwardSpec, PfFormError> {
        use crate::config::{ForwardMode, ForwardSpec};
        // Trim defensively even though input filtering already blocks
        // whitespace, so any value is persisted clean. `u16::parse` enforces
        // the 0..=65535 range; port 0 ("let kernel pick") is accepted.
        let listen_port: u16 = self
            .field_text(PfField::ListenPort)
            .trim()
            .parse()
            .map_err(|_| PfFormError::ListenPortRange)?;
        let bind_raw = self.field_text(PfField::BindAddr).trim();
        let bind_addr = if bind_raw.is_empty() {
            None
        } else {
            Some(bind_raw.to_string())
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
    pub host: String,
    pub selected: usize,
    pub add_form: Option<PfAddForm>,
    pub status: Option<String>,
}
