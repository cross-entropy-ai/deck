//! Startup/teardown guards: RAII and preflight checks that gate deck around
//! the TUI run — a single-instance lock, the pre-TUI environment preflight,
//! and the terminal raw-mode enter/restore guard.
pub mod instance_guard;
pub mod preflight_guard;
pub mod terminal_guard;
