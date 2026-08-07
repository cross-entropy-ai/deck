pub mod agent;
pub mod command;
pub mod focus;
pub mod guards;
pub mod parser;
pub mod pty;
pub mod refresh;
pub mod self_update;
pub mod seqlog;
pub mod shutdown;
pub mod ssh;
pub mod summary;
pub mod tmux;
pub mod update;
pub mod worker;

/// Short one-line label for the two IO failures every path/dir check reports
/// identically. `None` means "not one of those" — the caller supplies its own
/// fallback wording.
pub fn io_error_label(kind: std::io::ErrorKind) -> Option<&'static str> {
    match kind {
        std::io::ErrorKind::NotFound => Some("not found"),
        std::io::ErrorKind::PermissionDenied => Some("permission denied"),
        _ => None,
    }
}
