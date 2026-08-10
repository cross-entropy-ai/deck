//! The Agents-tab "Summary" card: its generation state plus the runtime
//! fields driving rendering and interaction (scroll, drag, in-popup scroll,
//! pre-generation state kept for cancel), grouped into one [`SummaryCard`]
//! unit on `AppState`. Persisted summary settings (`summary_prompt` /
//! `summary_agent` / `summary_model` / `summary_height` / `summary_language`) are user prefs
//! in [`crate::state::Prefs`]; only per-run runtime state lives here.

use serde::{Deserialize, Serialize};

/// Headless coding-agent CLI used to generate the Agents-tab summary.
///
/// The serialized names are part of `config.yaml`, so keep them stable even
/// if the labels shown in Settings change later.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SummaryAgent {
    #[default]
    Claude,
    Codex,
}

impl SummaryAgent {
    pub const ALL: [Self; 2] = [Self::Claude, Self::Codex];

    pub fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude",
            Self::Codex => "Codex",
        }
    }
}

/// State of the Agents-tab "Summary" card. `Idle` shows a "Generate Summary"
/// button; clicking kicks an async job and flips to `Generating` (animated
/// placeholder); when the job finishes the text lands and it becomes `Ready`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SummaryState {
    #[default]
    Idle,
    Generating,
    Ready {
        text: String,
        /// Unix seconds when the text landed, for the card's "Xm ago" age and
        /// to drive its "Re-generate" affordance.
        generated_at: u64,
    },
    /// Generation failed (no agents, selected CLI missing, non-zero exit); the
    /// card shows the reason and the Generate button stays available to retry.
    Error(String),
}

/// Default body height (text rows) of the Agents-tab Summary card. The
/// live value is `state.prefs.summary_height`, drag-adjustable and persisted.
pub const DEFAULT_SUMMARY_HEIGHT: u16 = 6;
/// Drag-resize bounds for the summary card's body height.
pub const SUMMARY_MIN_HEIGHT: u16 = 2;
pub const SUMMARY_MAX_HEIGHT: u16 = 40;

/// The Agents-tab Summary card's runtime state, grouped into one unit.
/// Persisted settings (prompt/model/height/language) live in `Prefs`, not
/// here — this is the transient per-run state.
#[derive(Debug, Default)]
pub struct SummaryCard {
    pub state: SummaryState,
    /// The state captured just before flipping to `Generating`, so
    /// cancelling (Esc on the Agents tab) restores the prior Idle / Ready /
    /// Error card rather than leaving a half-finished `Generating`.
    pub before_generating: Option<SummaryState>,
    /// True while dragging the card's bottom edge to resize it.
    pub dragging: bool,
    /// Scroll offset (in wrapped text rows) of the Ready summary's content,
    /// when it overflows the card's fixed text area.
    pub scroll: usize,
    /// Scroll offset of the summary popup's text, and its captured max.
    pub popup_scroll: usize,
    pub popup_max_scroll: usize,
}
