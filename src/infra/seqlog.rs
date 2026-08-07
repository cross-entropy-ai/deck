//! Opt-in log of the escape sequences deck exchanges with terminals.
//!
//! Set `DECK_SEQ_LOG=/tmp/deck.log` to record them; unset, nothing is opened
//! and nothing is written. Debug aid for "which side isn't answering", since
//! terminals differ widely in which of these they implement.

use std::io::Write;

/// Append one line, with ESC shown as `\e` so the file stays readable.
pub fn log(line: &str) {
    let Ok(path) = std::env::var("DECK_SEQ_LOG") else {
        return;
    };
    let Ok(mut file) = std::fs::File::options()
        .create(true)
        .append(true)
        .open(path)
    else {
        return;
    };
    let _ = writeln!(file, "{}", line.replace('\x1b', "\\e"));
}

/// Render bytes for `log`, replacing anything non-UTF-8 rather than failing.
pub fn show(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}
