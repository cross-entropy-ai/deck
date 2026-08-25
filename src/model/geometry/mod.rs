//! Sidebar geometry: the pure layout and hit-testing shared by the UI
//! (drawing) and the model/action layers (hit-testing).
//!
//! It lives in `model`, not `ui`, so `ui` and `app` depend on the model for
//! geometry and never the reverse.
//!
//! - [`text`]: display-width string helpers and the sidebar's glyphs.
//! - [`tabs`]: the vertical layout's tab bar.
//! - [`areas`]: carving the frame into rects, and the layout pass's output.
//! - [`hits`]: the click registry and its resolver.
//!
//! Everything is re-exported here, so callers keep writing
//! `crate::geometry::truncate` without caring which half of the module it
//! came from.

pub mod areas;
pub mod hits;
pub mod tabs;
pub mod text;

pub use areas::*;
pub use hits::*;
pub use tabs::*;
pub use text::*;

#[cfg(test)]
#[path = "../../../tests/unit/model/geometry.rs"]
mod tests;
