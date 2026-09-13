//! The Buddy wire protocol, and the lookup tables the synthesizer needs.
//!
//! Everything here is pure and free of macOS types, so it compiles and is
//! tested on every platform even though only `synth.rs` above it can act on
//! the result. The shape is fixed by the shipped iPad client
//! (`buddy/Networking/NetworkService.swift`) and its Python predecessor
//! (`server/buddy_server.py`) — this is a port, not a redesign, so an
//! unrecognised field is dropped rather than rejected.

// The keycode and modifier tables exist for `synth.rs`, which only builds on
// macOS. They live here, portable and unit-tested, rather than inside the FFI
// module that CI never compiles — so off macOS they are legitimately unused.
// Narrowed to that platform so real dead code is still caught where it matters.
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use serde::Deserialize;

/// One message from the client. The client only ever sends; nothing travels
/// back except WebSocket pongs, which the transport answers on its own.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum BuddyMsg {
    /// A chord, or a burst of them: each step is pressed and released in turn.
    Key {
        #[serde(default)]
        steps: Vec<Step>,
    },
    /// Literal text to insert. Line breaks are Return presses, not `\n`.
    Text {
        #[serde(default)]
        text: String,
    },
    Mouse(Mouse),
}

/// One key press: a key name plus the modifiers held for it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Step {
    #[serde(default)]
    pub key: String,
    /// Absent on an unmodified key, and the Python read it as nullable, so
    /// both shapes have to land on "no modifiers".
    #[serde(default)]
    pub modifiers: Option<Vec<String>>,
}

impl Step {
    pub fn modifiers(&self) -> &[String] {
        self.modifiers.as_deref().unwrap_or(&[])
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Mouse {
    pub action: MouseAction,
    /// Whole-pixel deltas. The client keeps the sub-pixel remainder itself, so
    /// slow movement isn't quantised away before it gets here.
    #[serde(default)]
    pub dx: i64,
    #[serde(default)]
    pub dy: i64,
    #[serde(default)]
    pub button: MouseButton,
    #[serde(default = "one")]
    pub count: i64,
}

fn one() -> i64 {
    1
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MouseAction {
    Move,
    Scroll,
    Click,
    Down,
    Up,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MouseButton {
    #[default]
    Left,
    Right,
    Middle,
}

/// Decode one frame's payload. `None` for anything that isn't a message this
/// server knows — malformed JSON, an unknown `type`, an unknown mouse action.
/// Dropping is deliberate: a client from a newer app should degrade to "that
/// button does nothing", never to a dropped connection.
pub fn parse(raw: &[u8]) -> Option<BuddyMsg> {
    serde_json::from_slice(raw).ok()
}

// --- macOS virtual keycodes (Carbon `Events.h` kVK_*) -----------------------
//
// Kept as bare `u16` so this file stays platform-free. The synthesizer casts
// them to `CGKeyCode`, which is the same integer.

/// Named keys the client can send, from the Python's `SPECIAL_KEYS`. Matched
/// case-insensitively; both spellings of the aliased ones are live.
const SPECIAL_KEYS: &[(&str, u16)] = &[
    ("escape", 0x35),
    ("esc", 0x35),
    ("return", 0x24),
    ("enter", 0x24),
    ("up", 0x7E),
    ("down", 0x7D),
    ("left", 0x7B),
    ("right", 0x7C),
    ("space", 0x31),
    ("tab", 0x30),
    ("capslock", 0x39),
    ("delete", 0x33),
    ("backspace", 0x33),
    ("home", 0x73),
    ("end", 0x77),
    ("pageup", 0x74),
    ("pagedown", 0x79),
    ("f1", 0x7A),
    ("f2", 0x78),
    ("f3", 0x63),
    ("f4", 0x76),
    ("f5", 0x60),
    ("f6", 0x61),
    ("f7", 0x62),
    ("f8", 0x64),
    ("f9", 0x65),
    ("f10", 0x6D),
    ("f11", 0x67),
    ("f12", 0x6F),
    ("f13", 0x69),
    ("f14", 0x6B),
    ("f15", 0x71),
    ("f16", 0x6A),
    ("f17", 0x40),
    ("f18", 0x4F),
    ("f19", 0x50),
    ("f20", 0x5A),
];

/// The US/ANSI layout, needed only for a *modified* character: ⌘C has to be a
/// real C keypress, and the Unicode path that serves everything else cannot
/// carry modifiers. Wrong on a non-ANSI hardware layout — see the plan's
/// ceilings; `UCKeyTranslate` is the way out if anyone hits it.
const ANSI_KEYS: &[(char, u16)] = &[
    ('a', 0x00),
    ('s', 0x01),
    ('d', 0x02),
    ('f', 0x03),
    ('h', 0x04),
    ('g', 0x05),
    ('z', 0x06),
    ('x', 0x07),
    ('c', 0x08),
    ('v', 0x09),
    ('b', 0x0B),
    ('q', 0x0C),
    ('w', 0x0D),
    ('e', 0x0E),
    ('r', 0x0F),
    ('y', 0x10),
    ('t', 0x11),
    ('1', 0x12),
    ('2', 0x13),
    ('3', 0x14),
    ('4', 0x15),
    ('6', 0x16),
    ('5', 0x17),
    ('=', 0x18),
    ('9', 0x19),
    ('7', 0x1A),
    ('-', 0x1B),
    ('8', 0x1C),
    ('0', 0x1D),
    (']', 0x1E),
    ('o', 0x1F),
    ('u', 0x20),
    ('[', 0x21),
    ('i', 0x22),
    ('p', 0x23),
    ('l', 0x25),
    ('j', 0x26),
    ('\'', 0x27),
    ('k', 0x28),
    (';', 0x29),
    ('\\', 0x2A),
    (',', 0x2B),
    ('/', 0x2C),
    ('n', 0x2D),
    ('m', 0x2E),
    ('.', 0x2F),
    ('`', 0x32),
];

/// The keycode for `name`: a named special key, or a single character on the
/// ANSI layout. `None` for anything else — the caller then has the Unicode
/// path, which needs no keycode at all.
pub fn key_code(name: &str) -> Option<u16> {
    let lower = name.to_lowercase();
    if let Some((_, code)) = SPECIAL_KEYS.iter().find(|(n, _)| *n == lower) {
        return Some(*code);
    }
    let mut chars = lower.chars();
    let first = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    ANSI_KEYS
        .iter()
        .find(|(c, _)| *c == first)
        .map(|(_, code)| *code)
}

// --- CGEventFlags masks ----------------------------------------------------

pub const FLAG_SHIFT: u64 = 0x0002_0000;
pub const FLAG_CONTROL: u64 = 0x0004_0000;
pub const FLAG_ALTERNATE: u64 = 0x0008_0000;
pub const FLAG_COMMAND: u64 = 0x0010_0000;

/// The `CGEventFlags` bitmask for a step's modifier names, from the Python's
/// `MODIFIERS` table. Unknown names are ignored, as they were there.
pub fn modifier_flags(names: &[String]) -> u64 {
    names.iter().fold(0, |flags, name| {
        flags
            | match name.to_lowercase().as_str() {
                "command" | "cmd" => FLAG_COMMAND,
                "control" | "ctrl" => FLAG_CONTROL,
                "shift" => FLAG_SHIFT,
                "option" | "opt" | "alt" => FLAG_ALTERNATE,
                _ => 0,
            }
    })
}

/// `CGEventKeyboardSetUnicodeString` silently truncates a long payload, so text
/// goes out in pieces. The limit is in UTF-16 units because that is what the
/// API counts — an emoji costs two of them — and a character is never split.
pub const UNICODE_CHUNK_UNITS: usize = 20;

pub fn utf16_chunks(text: &str, limit: usize) -> Vec<&str> {
    let mut chunks = Vec::new();
    let mut start = 0;
    let mut units = 0;
    for (offset, ch) in text.char_indices() {
        let width = ch.len_utf16();
        if offset > start && units + width > limit {
            chunks.push(&text[start..offset]);
            start = offset;
            units = 0;
        }
        units += width;
    }
    if start < text.len() {
        chunks.push(&text[start..]);
    }
    chunks
}

#[cfg(test)]
#[path = "../../../tests/unit/infra/buddy_protocol.rs"]
mod tests;
