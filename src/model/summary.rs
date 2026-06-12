//! The Agents-tab "Summary" card: its generation state plus the loose
//! runtime fields that drive its rendering and interaction (scroll, drag,
//! the in-popup scroll, and the pre-generation state kept for cancel).
//!
//! Grouped into one [`SummaryCard`] unit on `AppState` so these related
//! fields travel together instead of being six exploded `summary_*` fields.
//! The *persisted* summary settings (`summary_prompt` / `summary_model` /
//! `summary_height` / `summary_language`) are user preferences and stay in
//! [`crate::state::Prefs`] — only the per-run runtime state lives here.

/// State of the "Summary" card at the top of the Agents tab. `Idle` shows
/// a "Generate Summary" button; clicking it kicks an async job and flips
/// to `Generating` (an animated placeholder); when the job finishes the
/// generated text lands and it becomes `Ready`.
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
    /// Generation failed (no agents, `claude` missing, non-zero exit); the
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
    /// Generation state (idle / generating / ready / error).
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
