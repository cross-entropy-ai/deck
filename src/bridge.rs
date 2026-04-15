use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

/// Render a vt100 virtual screen into a ratatui buffer region.
pub fn render_screen(screen: &vt100::Screen, area: Rect, buf: &mut Buffer) {
    for row in 0..area.height.min(screen.size().0) {
        for col in 0..area.width.min(screen.size().1) {
            let Some(cell) = screen.cell(row, col) else {
                continue;
            };
            if cell.is_wide_continuation() {
                continue;
            }

            let x = area.x + col;
            let y = area.y + row;
            let Some(target) = buf.cell_mut((x, y)) else {
                continue;
            };

            let contents = cell.contents();
            if contents.is_empty() {
                target.set_char(' ');
            } else {
                target.set_symbol(contents);
            }

            let fg = convert_color(cell.fgcolor());
            let bg = convert_color(cell.bgcolor());
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

fn convert_color(c: vt100::Color) -> Color {
    match c {
        vt100::Color::Default => Color::Reset,
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
