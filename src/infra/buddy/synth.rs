//! Turning a [`BuddyMsg`] into real macOS input events.
//!
//! A port of the Python's `KeyDispatcher` and `type_literal`. Each of the
//! non-obvious rules below was a bug there first; the comments say which.

use core_graphics::display::CGDisplay;
use core_graphics::event::{
    CGEvent, CGEventFlags, CGEventTapLocation, CGEventType, CGMouseButton, EventField,
    ScrollEventUnit,
};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use core_graphics::geometry::CGPoint;

use super::protocol::{
    key_code, modifier_flags, utf16_chunks, BuddyMsg, Mouse, MouseAction, MouseButton, Step,
    UNICODE_CHUNK_UNITS,
};
use super::sink::InputSink;

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrusted() -> bool;
}

/// Whether this process may post events at all. Without the Accessibility
/// grant `CGEventPost` succeeds and does nothing, so the Settings page asks
/// this rather than leaving the user with input that silently vanishes.
///
/// The grant belongs to the terminal app running deck, not to deck.
pub fn accessibility_trusted() -> bool {
    unsafe { AXIsProcessTrusted() }
}

/// Posts synthesized events, and remembers the one piece of state a stream of
/// messages needs: which button is down, so that a move is sent as a drag.
pub struct Synth {
    pressed_button: Option<CGMouseButton>,
}

impl Default for Synth {
    fn default() -> Self {
        Self::new()
    }
}

impl Synth {
    pub fn new() -> Self {
        Self {
            pressed_button: None,
        }
    }

    fn mouse(&mut self, msg: &Mouse) {
        match msg.action {
            MouseAction::Move => self.move_by(msg.dx as f64, msg.dy as f64),
            MouseAction::Scroll => scroll(msg.dx as i32, msg.dy as i32),
            MouseAction::Click => click(cg_button(msg.button), msg.count.clamp(1, 3)),
            MouseAction::Down => {
                let button = cg_button(msg.button);
                self.pressed_button = Some(button);
                button_event(button, true, 1);
            }
            MouseAction::Up => {
                button_event(cg_button(msg.button), false, 1);
                self.pressed_button = None;
            }
        }
    }

    /// Move the pointer by (dx, dy), carrying the delta on the event.
    ///
    /// Two things here are not the obvious implementation:
    ///
    /// A held button has to move as a *drag* event, or the drag doesn't track.
    ///
    /// And the position is clamped to the desktop before posting, while the
    /// delta fields stay at what was asked for. The reported cursor location is
    /// whatever we last posted, not where the WindowServer actually put the
    /// cursor, so an off-screen point compounds: push left for a second and the
    /// reported position runs thousands of pixels past the edge while the
    /// visible cursor sits at 0, and the pointer then ignores you until you
    /// swipe the whole debt back. Keeping the unclamped delta means pressing
    /// outward at an edge still registers as movement for anything watching
    /// raw motion.
    fn move_by(&self, dx: f64, dy: f64) {
        let Some(current) = cursor_position() else {
            return;
        };
        let (min_x, min_y, max_x, max_y) = desktop_bounds();
        let target = CGPoint::new(
            (current.x + dx).clamp(min_x, max_x - 1.0),
            (current.y + dy).clamp(min_y, max_y - 1.0),
        );
        let (event_type, button) = match self.pressed_button {
            None => (CGEventType::MouseMoved, CGMouseButton::Left),
            Some(CGMouseButton::Left) => (CGEventType::LeftMouseDragged, CGMouseButton::Left),
            Some(CGMouseButton::Right) => (CGEventType::RightMouseDragged, CGMouseButton::Right),
            Some(other) => (CGEventType::OtherMouseDragged, other),
        };
        let Some(event) =
            source().and_then(|s| CGEvent::new_mouse_event(s, event_type, target, button).ok())
        else {
            return;
        };
        event.set_integer_value_field(EventField::MOUSE_EVENT_DELTA_X, dx as i64);
        event.set_integer_value_field(EventField::MOUSE_EVENT_DELTA_Y, dy as i64);
        event.post(CGEventTapLocation::HID);
    }
}

impl InputSink for Synth {
    fn dispatch(&mut self, msg: &BuddyMsg) {
        match msg {
            BuddyMsg::Key { steps } => steps.iter().for_each(press),
            BuddyMsg::Text { text } => type_text(text),
            BuddyMsg::Mouse(mouse) => self.mouse(mouse),
        }
    }

    /// Let go of a button this connection left held.
    ///
    /// Without this a connection that drops mid-drag — a Wi-Fi blip, the
    /// server being switched off, the approval being withdrawn — leaves the
    /// physical button down for the whole system, and nothing in deck can
    /// release it. The `up` has to go out on *every* exit path, not just the
    /// clean one.
    fn release_all(&mut self) {
        if let Some(button) = self.pressed_button.take() {
            button_event(button, false, 1);
        }
    }
}

fn source() -> Option<CGEventSource> {
    CGEventSource::new(CGEventSourceStateID::HIDSystemState).ok()
}

fn cg_button(button: MouseButton) -> CGMouseButton {
    match button {
        MouseButton::Left => CGMouseButton::Left,
        MouseButton::Right => CGMouseButton::Right,
        MouseButton::Middle => CGMouseButton::Center,
    }
}

/// Where the pointer is now, in CoreGraphics global space (origin top-left).
/// An event created with no type carries the current location.
fn cursor_position() -> Option<CGPoint> {
    CGEvent::new(source()?).ok().map(|event| event.location())
}

/// The union of every active display, in the same space.
fn desktop_bounds() -> (f64, f64, f64, f64) {
    let rects: Vec<_> = CGDisplay::active_displays()
        .unwrap_or_default()
        .into_iter()
        .map(|id| CGDisplay::new(id).bounds())
        .collect();
    if rects.is_empty() {
        // No displays to clamp against; let the move through unchanged rather
        // than pinning the cursor to the origin.
        return (f64::MIN, f64::MIN, f64::MAX, f64::MAX);
    }
    let fold = |f: fn(f64, f64) -> f64, pick: fn(&core_graphics::geometry::CGRect) -> f64| {
        rects.iter().map(pick).fold(f64::NAN, f)
    };
    (
        fold(f64::min, |r| r.origin.x),
        fold(f64::min, |r| r.origin.y),
        fold(f64::max, |r| r.origin.x + r.size.width),
        fold(f64::max, |r| r.origin.y + r.size.height),
    )
}

fn scroll(dx: i32, dy: i32) {
    let Some(event) = source()
        .and_then(|s| CGEvent::new_scroll_event(s, ScrollEventUnit::LINE, 2, dy, dx, 0).ok())
    else {
        return;
    };
    event.post(CGEventTapLocation::HID);
}

fn click(button: CGMouseButton, count: i64) {
    // Each press carries its index in `click_state`, which is how the window
    // server recognises a double-click rather than two singles.
    for index in 1..=count {
        button_event(button, true, index);
        button_event(button, false, index);
    }
}

fn button_event(button: CGMouseButton, down: bool, click_state: i64) {
    let event_type = match (button, down) {
        (CGMouseButton::Left, true) => CGEventType::LeftMouseDown,
        (CGMouseButton::Left, false) => CGEventType::LeftMouseUp,
        (CGMouseButton::Right, true) => CGEventType::RightMouseDown,
        (CGMouseButton::Right, false) => CGEventType::RightMouseUp,
        (_, true) => CGEventType::OtherMouseDown,
        (_, false) => CGEventType::OtherMouseUp,
    };
    let Some(point) = cursor_position() else {
        return;
    };
    let Some(event) =
        source().and_then(|s| CGEvent::new_mouse_event(s, event_type, point, button).ok())
    else {
        return;
    };
    event.set_integer_value_field(EventField::MOUSE_EVENT_CLICK_STATE, click_state);
    event.post(CGEventTapLocation::HID);
}

/// Press and release one step.
///
/// A key with modifiers, and every named special key, goes out as a real
/// keycode with the modifier mask set on the event — the Python pressed the
/// modifier keys themselves, but consumers read the flags. An unmodified single
/// character takes the Unicode path instead, for the reason `type_text`
/// explains.
fn press(step: &Step) {
    if step.key.is_empty() {
        return;
    }
    let flags = modifier_flags(step.modifiers());
    match key_code(&step.key) {
        Some(code) if flags != 0 || is_named_key(&step.key) => tap(code, flags),
        _ => type_text(&step.key),
    }
}

/// Whether the name is a special key rather than a character, so that a bare
/// `{"key":"a"}` takes the Unicode path while `{"key":"escape"}` does not.
fn is_named_key(name: &str) -> bool {
    name.chars().count() > 1
}

fn tap(code: u16, flags: u64) {
    let Some(source) = source() else { return };
    for down in [true, false] {
        let Ok(event) = CGEvent::new_keyboard_event(source.clone(), code, down) else {
            return;
        };
        if flags != 0 {
            event.set_flags(CGEventFlags::from_bits_truncate(flags));
        }
        event.post(CGEventTapLocation::HID);
    }
}

/// Insert `text` verbatim, bypassing whatever input source is active.
///
/// Pressing the key mapped to a character would let an active IME (拼音 and
/// friends) swallow it into its composition buffer instead of passing it
/// through. Posting keycode 0 with a Unicode payload hands the characters
/// straight to the focused app and never reaches the input method.
///
/// A literal newline would submit nothing in a terminal, so line breaks stay
/// real Return presses.
fn type_text(text: &str) {
    for (index, line) in text.split('\n').enumerate() {
        if index > 0 {
            tap(0x24, 0);
        }
        for chunk in utf16_chunks(line, UNICODE_CHUNK_UNITS) {
            let units: Vec<u16> = chunk.encode_utf16().collect();
            let Some(source) = source() else { return };
            for down in [true, false] {
                let Ok(event) = CGEvent::new_keyboard_event(source.clone(), 0, down) else {
                    return;
                };
                event.set_string_from_utf16_unchecked(&units);
                event.post(CGEventTapLocation::HID);
            }
        }
    }
}
