# Known Bugs

## Bug 1: `session_at_row` footer height mismatch

**File:** `src/state.rs:290`

The footer layout allocates 3 rows (`Constraint::Length(3)` in `ui.rs:101`), and `draw_footer` renders 3 lines (separator + hints + info). But `session_at_row` hardcodes `footer_height = 2u16`. This makes the clickable session area 1 row taller than the actual rendered area, so clicks near the bottom of the sidebar map to the wrong session or accept out-of-bounds clicks.

**Fix:** Change `footer_height` from `2` to `3`.

---

## Bug 2: Exclude editor cursor broken for multi-byte characters

**File:** `src/action.rs:420, 429`

`ExcludeEditorInput` advances the cursor by 1 byte (`editor.cursor += 1`) instead of `ch.len_utf8()`. `ExcludeEditorBackspace` retreats by 1 byte (`editor.cursor -= 1`) instead of finding the previous char boundary. If a non-ASCII character is typed, the cursor lands mid-char and `String::remove` panics.

Compare with the correct `RenameInput`/`RenameBackspace` handlers which use `ch.len_utf8()` and scan for the previous char boundary.

**Fix:** Use `ch.len_utf8()` for input, and replicate the `RenameBackspace` char-boundary logic for backspace.

---

## Bug 3: `draw_rename_input` slices strings by bytes, not display width

**File:** `src/ui.rs:752-780`

- `input.len()` (byte length) is compared against `max_w` (display width). The slice `&input[input.len() - max_w..]` can land mid-char and panic.
- `&after[..1]` and `&after[1..]` take 1 byte, not 1 character. If the cursor sits on a multi-byte character, this panics.

**Fix:** Use char-aware iteration or `UnicodeWidthStr` for width calculation and char-boundary-safe slicing.

---

## Bug 4: `resize_sidebar` / `resize_sidebar_height` u16 underflow

**File:** `src/state.rs:428, 438`

```rust
SIDEBAR_MAX.min(self.term_width - 10)    // panics if term_width < 10
SIDEBAR_HEIGHT_MAX.min(self.term_height - 6)  // panics if term_height < 6
```

Unsigned `u16` subtraction panics in debug mode and wraps to ~65530 in release mode when the terminal is very small.

**Fix:** Use `saturating_sub`.

---

## Bug 5: `tab_col_ranges` uses byte length for column width

**File:** `src/ui.rs:680`

```rust
let name_width = session.name.len() as u16;
```

Uses byte length instead of display width (`UnicodeWidthStr::width()`). Session names with CJK characters, emoji, or other wide/multi-byte chars produce wrong column ranges, making tab clicks in vertical mode map to the wrong session.

**Fix:** Use `session.name.width() as u16`.

---

## Bug 6: `parse_string_array` splits on commas inside values

**File:** `src/config.rs:218`

```rust
s.split(',')
```

A regex exclude pattern like `/foo,bar/` gets split into `/foo` and `bar/`, corrupting the pattern on config reload.

**Fix:** Parse quoted strings properly instead of naive comma splitting.

---

## Bug 7: Navigation at boundary triggers unnecessary session switches

**File:** `src/action.rs:109-142`

`FocusNext` at the last item, `FocusPrev` at index 0, and `ScrollUp`/`ScrollDown` at boundaries all emit `fx.switch_session` even though `focused` didn't change. This causes a redundant `tmux switch-client` call on every boundary keypress, which can produce visible flicker.

**Fix:** Only set `fx.switch_session` when `focused` actually changed.

---

## Bug 8: JSON `quote()` doesn't escape control characters

**File:** `src/config.rs:89-91`

Only backslashes and double-quotes are escaped. Newlines, tabs, and other control characters in pattern strings produce invalid JSON, breaking the config file on reload.

**Fix:** Also escape `\n`, `\r`, `\t`, and other control characters (`\u00XX`).
