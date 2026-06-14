use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

/// Render a vt100 virtual screen into a ratatui buffer region.
/// `default_fg`/`default_bg` are used for cells with no explicit color
/// (vt100::Color::Default), so the terminal content follows the deck theme.
pub fn render_screen(
    screen: &vt100::Screen,
    area: Rect,
    buf: &mut Buffer,
    default_fg: Color,
    default_bg: Color,
) {
    for row in 0..area.height.min(screen.size().0) {
        for col in 0..area.width.min(screen.size().1) {
            let Some(cell) = screen.cell(row, col) else {
                continue;
            };

            let x = area.x + col;
            let y = area.y + row;
            let Some(target) = buf.cell_mut((x, y)) else {
                continue;
            };

            if cell.is_wide_continuation() {
                // The previous column rendered a 2-cell wide glyph that
                // covers this position. Mark skip=true so ratatui's diff
                // knows this cell is owned by the wide glyph and
                // (critically) so the diff can correctly overwrite this
                // position when the previous frame had different content
                // here.
                //
                // Without this, residue appears in two cases:
                // - Session switch (covered by terminal.clear() in
                //   render.rs).
                // - Sidebar resize (skip=true is the only guard).
                target.set_skip(true);
                continue;
            }

            let contents = cell.contents();
            if contents.is_empty() {
                target.set_char(' ');
            } else {
                target.set_symbol(contents);
            }

            let fg = convert_color(cell.fgcolor(), default_fg);
            let bg = convert_color(cell.bgcolor(), default_bg);
            let mut modifier = Modifier::empty();
            if cell.bold() {
                modifier |= Modifier::BOLD;
            }
            if cell.underline() {
                modifier |= Modifier::UNDERLINED;
            }
            if cell.italic() {
                modifier |= Modifier::ITALIC;
            }

            let style = if cell.inverse() {
                Style::default().fg(bg).bg(fg).add_modifier(modifier)
            } else {
                Style::default().fg(fg).bg(bg).add_modifier(modifier)
            };
            target.set_style(style);
        }
    }
}

fn convert_color(c: vt100::Color, default: Color) -> Color {
    match c {
        vt100::Color::Default => default,
        vt100::Color::Idx(i) => Color::Indexed(i),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

/// Set the terminal cursor position to match the vt100 cursor,
/// offset into the main pane area. Only meaningful when main pane is focused.
pub fn set_cursor(frame: &mut ratatui::Frame, screen: &vt100::Screen, area: Rect) {
    let (row, col) = screen.cursor_position();
    let x = area.x + col;
    let y = area.y + row;
    if x < area.right() && y < area.bottom() {
        frame.set_cursor_position((x, y));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_char_continuation_sets_skip_flag() {
        // CJK ideograph U+4E2D ("中") is a typical 2-cell-wide glyph.
        let mut parser = vt100::Parser::new(2, 10, 0);
        parser.process("中".as_bytes());

        let area = Rect::new(0, 0, 10, 2);
        let mut buf = Buffer::empty(area);
        render_screen(parser.screen(), area, &mut buf, Color::White, Color::Black);

        // Column 0 holds the wide char itself; column 1 is its
        // continuation. Without the skip flag, ratatui's diff cannot
        // tell column 1 is "owned by" column 0, and residue from a
        // previous frame can leak through.
        let col_0 = buf.cell((0, 0)).unwrap();
        let col_1 = buf.cell((1, 0)).unwrap();
        assert_eq!(col_0.symbol(), "中");
        assert!(
            col_1.skip,
            "wide-char continuation cell must have skip=true"
        );
    }
}
