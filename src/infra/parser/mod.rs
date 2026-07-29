//! Pure parsers for backend command output — text in, structs out, no I/O.
//! What lives here is *shared* by the local and remote tmux backends (CLAUDE.md:
//! push the local/remote split as low as it goes); shelling-out, quoting, and
//! error semantics stay in each owning module. A parser with a single caller
//! lives in that caller instead.
pub mod pane;
pub mod tmux;
