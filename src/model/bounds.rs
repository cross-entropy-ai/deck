//! Moving and clamping a value inside a range.
//!
//! Pure arithmetic with no state of its own: stepping a list cursor, wrapping
//! through a fixed set of options, applying a scroll delta, clamping a resize.
//! Shared by the settings cyclers, every picker, the port-forward form, the
//! summary card, and the drag-resize setters — which is why these lived in
//! `state` for a while, though none of them knows what `AppState` is.

/// Step `delta` positions through `options` from `current`, wrapping at both
/// ends; a `current` not in the slice steps from the first option. Shared by
/// the settings cyclers and the port-forward form's field/mode cycling.
pub fn cycle_option<T: Copy + PartialEq>(options: &[T], current: T, delta: i32) -> T {
    let i = options.iter().position(|&o| o == current).unwrap_or(0) as i32;
    let n = options.len() as i32;
    options[(i + delta).rem_euclid(n) as usize]
}

/// Step `current` by `direction` (+1/-1) within `0..len`, clamped at both
/// ends (no wrap); `len == 0` yields 0. Shared by the bounded list cursors
/// (settings rows, theme picker, exclude editor, port-forward focus, pickers).
pub fn step_clamped(current: usize, len: usize, direction: i32) -> usize {
    let Some(last) = len.checked_sub(1) else {
        return 0;
    };
    if direction >= 0 {
        current.saturating_add(direction as usize).min(last)
    } else {
        // Backward steps don't clamp to `last`: an already out-of-range cursor
        // walks back into range rather than jumping there.
        current.saturating_sub(direction.unsigned_abs() as usize)
    }
}

/// Apply a scroll `delta` to `current`, clamped to `0..=max`. Shared by the
/// summary card and its popup so the i32 round-trip lives in one place.
pub fn scroll_clamped(current: usize, delta: i32, max: usize) -> usize {
    (current as i32 + delta).clamp(0, max as i32) as usize
}

/// Clamp `value` to `min..=max` and store it in `*target` if it differs,
/// returning whether `*target` changed. Shared by the drag-resize setters.
pub fn clamp_set(target: &mut u16, value: u16, min: u16, max: u16) -> bool {
    let clamped = value.clamp(min, max);
    let changed = clamped != *target;
    *target = clamped;
    changed
}

/// Clamp `*cursor` to the last valid index of a `len`-length list (0 when
/// empty). Shared by the Projects/Agents focus clamps.
pub fn clamp_cursor(cursor: &mut usize, len: usize) {
    *cursor = (*cursor).min(len.saturating_sub(1));
}

#[cfg(test)]
#[path = "../../tests/unit/model/bounds.rs"]
mod tests;
