//! Ask the host terminal what its own background color is (OSC 11) so the
//! "follow terminal" theme mode can choose between a dark and a light theme.
//!
//! The query goes to `/dev/tty` and the answer comes back on the same tty as
//! input, which means this must run while nothing else is reading terminal
//! input — at startup, or between event-loop polls. See `App::probe_terminal_bg`.

use std::fs::File;
use std::io::{Read, Write};
use std::os::unix::io::AsRawFd;
use std::time::{Duration, Instant};

/// How long to wait for the terminal to answer. Terminals that support the
/// query answer in about a millisecond; the rest never answer at all, so this
/// is a floor on startup cost only for those.
pub const PROBE_TIMEOUT: Duration = Duration::from_millis(100);

/// Probe the host terminal's background color and report whether it's dark.
/// `None` = no usable answer (terminal doesn't implement OSC 11, isn't a tty,
/// or timed out) — callers keep whatever they assumed before.
pub fn terminal_is_dark(timeout: Duration) -> Option<bool> {
    let reply = query(b"\x1b]11;?\x1b\\", timeout)?;
    let payload = osc_payload(reply.as_bytes(), 11)?;
    // Mid-gray splits the two: everything darker reads as a dark terminal.
    Some(perceived_luma(payload)? < 0.5)
}

/// Write `query` to the tty and read whatever it answers, with the tty in raw
/// mode so the reply doesn't get line-buffered or echoed.
fn query(query: &[u8], timeout: Duration) -> Option<String> {
    let tty = File::options()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .ok()?;
    let fd = tty.as_raw_fd();

    let mut saved: libc::termios = unsafe { std::mem::zeroed() };
    if unsafe { libc::tcgetattr(fd, &mut saved) } != 0 {
        return None;
    }
    let mut raw = saved;
    unsafe { libc::cfmakeraw(&mut raw) };
    // VMIN 0 + VTIME makes each read() return empty after the timeout instead
    // of blocking forever on a terminal that never answers. VTIME is in
    // deciseconds, so a sub-100ms timeout still costs one decisecond.
    raw.c_cc[libc::VMIN] = 0;
    raw.c_cc[libc::VTIME] = (timeout.as_millis() / 100).clamp(1, 255) as u8;
    if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw) } != 0 {
        return None;
    }

    let reply = read_reply(&tty, query, timeout);
    unsafe { libc::tcsetattr(fd, libc::TCSANOW, &saved) };
    reply
}

// ponytail: whatever the terminal sends during the probe window is treated as
// the reply, so a keystroke typed in those few ms is swallowed instead of
// reaching the pane. Filtering it back out means buffering unrelated input and
// re-injecting it — worth doing only if the probe ever runs on a timer.
fn read_reply(mut tty: &File, query: &[u8], timeout: Duration) -> Option<String> {
    tty.write_all(query).ok()?;
    tty.flush().ok()?;

    let deadline = Instant::now() + timeout;
    let mut reply = Vec::new();
    let mut chunk = [0u8; 64];
    while Instant::now() < deadline {
        match tty.read(&mut chunk) {
            // A zero-length read is the VTIME timeout firing: nothing coming.
            Ok(0) => break,
            Ok(n) => reply.extend_from_slice(&chunk[..n]),
            Err(_) => return None,
        }
        // Stop as soon as the OSC string is terminated (BEL or ST); waiting for
        // the full deadline would add the timeout to every startup.
        if reply.contains(&0x07) || reply.windows(2).any(|w| w == b"\x1b\\") {
            break;
        }
    }
    (!reply.is_empty()).then(|| String::from_utf8_lossy(&reply).into_owned())
}

/// The payload of an `OSC <code>;<payload>` string in `data`, without its
/// terminator. `None` if `data` holds no such reply.
fn osc_payload(data: &[u8], code: u16) -> Option<&str> {
    let marker = format!("\x1b]{code};");
    let start = data
        .windows(marker.len())
        .position(|w| w == marker.as_bytes())?
        + marker.len();
    let rest = &data[start..];
    let end = rest
        .iter()
        .position(|&b| b == 0x07 || b == 0x1b)
        .unwrap_or(rest.len());
    std::str::from_utf8(&rest[..end]).ok()
}

/// Perceived brightness (0.0–1.0) of an X color spec — `rgb:R/G/B` with 1–4
/// hex digits per channel (what terminals answer), or `#rrggbb`.
fn perceived_luma(spec: &str) -> Option<f64> {
    let channels: Vec<f64> = match spec.trim().strip_prefix("rgb:") {
        Some(rest) => rest.split('/').map(hex_fraction).collect::<Option<_>>()?,
        None => {
            let hex = spec.trim().strip_prefix('#')?;
            if hex.len() != 6 {
                return None;
            }
            (0..3)
                .map(|i| hex_fraction(&hex[i * 2..i * 2 + 2]))
                .collect::<Option<_>>()?
        }
    };
    let [r, g, b] = channels[..] else { return None };
    // Rec. 601 luma — the same weighting `ui::menu` uses for contrast checks.
    Some(0.299 * r + 0.587 * g + 0.114 * b)
}

/// One hex channel as a 0.0–1.0 fraction, scaled by its own width so `ff` and
/// `ffff` both mean "full".
fn hex_fraction(hex: &str) -> Option<f64> {
    if hex.is_empty() || hex.len() > 4 {
        return None;
    }
    let value = u32::from_str_radix(hex, 16).ok()?;
    let max = 16u32.pow(hex.len() as u32) - 1;
    Some(value as f64 / max as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_dark(reply: &[u8]) -> Option<bool> {
        let payload = osc_payload(reply, 11)?;
        Some(perceived_luma(payload)? < 0.5)
    }

    #[test]
    fn classifies_real_terminal_replies() {
        // Ghostty/iTerm answer 4 digits per channel and BEL- or ST-terminate.
        assert_eq!(is_dark(b"\x1b]11;rgb:1e1e/1e1e/2e2e\x1b\\"), Some(true));
        assert_eq!(is_dark(b"\x1b]11;rgb:eeee/f1f1/f5f5\x07"), Some(false));
        // Some terminals use fewer digits, or the #rrggbb form.
        assert_eq!(is_dark(b"\x1b]11;rgb:00/00/00\x07"), Some(true));
        assert_eq!(is_dark(b"\x1b]11;#ffffff\x07"), Some(false));
        // Green is bright enough to read light even though R and B are zero —
        // that's the luma weighting, not a bug.
        assert_eq!(is_dark(b"\x1b]11;rgb:0000/ffff/0000\x07"), Some(false));
    }

    #[test]
    fn rejects_junk_instead_of_guessing() {
        // No reply, a different OSC code, and an unparseable payload must all
        // fall through to "unknown" so the caller keeps its current theme.
        assert_eq!(is_dark(b"garbage from a chatty shell"), None);
        assert_eq!(is_dark(b"\x1b]10;rgb:1e1e/1e1e/2e2e\x1b\\"), None);
        assert_eq!(is_dark(b"\x1b]11;mauve\x07"), None);
        assert_eq!(is_dark(b"\x1b]11;rgb:1e1e/1e1e\x07"), None);
        assert_eq!(is_dark(b"\x1b]11;rgb:zzzz/1e1e/2e2e\x07"), None);
    }
}
