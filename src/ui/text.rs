use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::keybindings::{format_key, Command, Keybindings};
use crate::theme::Theme;

pub(super) fn pad_line<'a>(
    spans: Vec<Span<'a>>,
    bg: ratatui::style::Color,
    width: usize,
) -> Line<'a> {
    let mut line = Line::from(spans);
    let content_width = line.width();
    if content_width < width {
        line.spans.push(Span::styled(
            " ".repeat(width - content_width),
            Style::default().bg(bg),
        ));
    }
    line
}

/// Inline/line style of a run produced by `wrap_markdown`. The summary is
/// rendered from a tiny markdown subset: `**bold**`, `` `code` ``, and
/// `#`-prefixed headings (code fences / tables are disallowed by prompt).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum MdStyle {
    Plain,
    Bold,
    Code,
    Heading,
}

/// One run = a contiguous slice of text sharing a style.
pub(super) type MdRun = (String, MdStyle);

/// Word-wrap `text` to `width` columns, parsing the markdown subset above.
/// Returns one entry per display line; each line is a list of styled runs.
/// Markers (`**`, `` ` ``, leading `#`s) are consumed, not rendered, and
/// hard `\n`s force breaks.
// The `flush_word!` macro resets `line_w`/`word_w` on its final expansion
// where the resets are dead — expected, not a bug.
#[allow(unused_assignments)]
pub(super) fn wrap_markdown(text: &str, width: usize) -> Vec<Vec<MdRun>> {
    let width = width.max(1);

    // 1) Strip markers into per-char styles. `**` toggles bold, `` ` ``
    //    toggles code, and a logical line starting with `#`+space is a
    //    heading (the rest of that line). Bold/code/heading reset at `\n`.
    let cs: Vec<char> = text.chars().collect();
    let mut styled: Vec<(char, MdStyle)> = Vec::new();
    let mut bold = false;
    let mut code = false;
    let mut heading = false;
    let mut line_start = true;
    let mut i = 0;
    while i < cs.len() {
        let c = cs[i];
        if line_start {
            line_start = false;
            let mut j = i;
            while j < cs.len() && cs[j] == '#' {
                j += 1;
            }
            if j > i && cs.get(j) == Some(&' ') {
                heading = true;
                i = j + 1; // skip the `#`s and the single space
                continue;
            }
        }
        if c == '\n' {
            styled.push(('\n', MdStyle::Plain));
            bold = false;
            code = false;
            heading = false;
            line_start = true;
            i += 1;
            continue;
        }
        if !code && c == '*' && cs.get(i + 1) == Some(&'*') {
            bold = !bold;
            i += 2;
            continue;
        }
        if c == '`' {
            code = !code;
            i += 1;
            continue;
        }
        let style = if heading {
            MdStyle::Heading
        } else if code {
            MdStyle::Code
        } else if bold {
            MdStyle::Bold
        } else {
            MdStyle::Plain
        };
        styled.push((c, style));
        i += 1;
    }

    // 2) Greedy word-wrap, carrying each char's style.
    let cw = |c: char| UnicodeWidthChar::width(c).unwrap_or(0);
    let mut lines: Vec<Vec<(char, MdStyle)>> = Vec::new();
    let mut line: Vec<(char, MdStyle)> = Vec::new();
    let mut line_w = 0usize;
    let mut word: Vec<(char, MdStyle)> = Vec::new();
    let mut word_w = 0usize;

    macro_rules! flush_word {
        () => {{
            if !word.is_empty() {
                if line_w > 0 && line_w + word_w > width {
                    lines.push(std::mem::take(&mut line));
                    line_w = 0;
                }
                if word_w > width {
                    // A single word longer than the line: hard-split it.
                    for (c, st) in word.drain(..) {
                        let w = cw(c);
                        if line_w > 0 && line_w + w > width {
                            lines.push(std::mem::take(&mut line));
                            line_w = 0;
                        }
                        line.push((c, st));
                        line_w += w;
                    }
                } else {
                    line.append(&mut word);
                    line_w += word_w;
                }
                word_w = 0;
            }
        }};
    }

    for (c, st) in styled {
        if c == '\n' {
            flush_word!();
            lines.push(std::mem::take(&mut line));
            line_w = 0;
        } else if c == ' ' {
            flush_word!();
            if line_w > 0 && line_w < width {
                line.push((' ', st));
                line_w += 1;
            }
        } else {
            word.push((c, st));
            word_w += cw(c);
        }
    }
    flush_word!();
    if !line.is_empty() || lines.is_empty() {
        lines.push(line);
    }

    // 3) Coalesce adjacent same-style chars into runs.
    lines
        .into_iter()
        .map(|chs| {
            let mut runs: Vec<MdRun> = Vec::new();
            for (c, st) in chs {
                match runs.last_mut() {
                    Some((s, rst)) if *rst == st => s.push(c),
                    _ => runs.push((c.to_string(), st)),
                }
            }
            runs
        })
        .collect()
}

/// Build styled spans for one `wrap_markdown` line: plain text takes
/// `base`; bold/heading use the accent highlight (heading also
/// underlined), inline code the teal highlight.
pub(super) fn md_line_spans(runs: &[MdRun], theme: &Theme, base: Style) -> Vec<Span<'static>> {
    use ratatui::style::Modifier;
    runs.iter()
        .map(|(seg, style)| {
            let s = match style {
                MdStyle::Plain => base,
                MdStyle::Bold => base.fg(theme.accent).add_modifier(Modifier::BOLD),
                MdStyle::Heading => base
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
                MdStyle::Code => base.fg(theme.teal),
            };
            Span::styled(seg.clone(), s)
        })
        .collect()
}

/// Total display width of a `wrap_markdown` line's runs.
pub(super) fn md_line_width(runs: &[MdRun]) -> usize {
    runs.iter().map(|(s, _)| s.width()).sum()
}

pub(super) fn format_keys_for(keybindings: &Keybindings, cmd: Command) -> String {
    keybindings
        .keys_for(cmd)
        .iter()
        .map(format_key)
        .collect::<Vec<_>>()
        .join("/")
}

pub(super) fn primary_key_string(keybindings: &Keybindings, cmd: Command) -> String {
    keybindings
        .keys_for(cmd)
        .first()
        .map(format_key)
        .unwrap_or_default()
}

pub(super) fn truncate(s: &str, max_width: usize) -> String {
    if s.width() <= max_width {
        return s.to_string();
    }
    if max_width <= 1 {
        return ".".to_string();
    }
    let mut out = String::new();
    let mut width = 0usize;

    for ch in s.chars() {
        let ch_width = ch.width().unwrap_or(0);
        if width + ch_width + 1 > max_width {
            break;
        }
        out.push(ch);
        width += ch_width;
    }

    format!("{out}…")
}

pub(super) fn shorten_dir(dir: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    if !home.is_empty() && dir.starts_with(&home) {
        format!("~{}", &dir[home.len()..])
    } else {
        dir.to_string()
    }
}

pub(super) fn format_idle_badge(seconds: u64) -> Option<String> {
    if seconds < 60 {
        return None;
    }
    if seconds < 3600 {
        return Some(format!("{}m", seconds / 60));
    }
    if seconds < 86_400 {
        return Some(format!("{}h", seconds / 3600));
    }
    Some(format!("{}d", seconds / 86_400))
}

pub(super) fn idle_color(
    theme: &Theme,
    idle_seconds: u64,
    emphasized: bool,
) -> ratatui::style::Color {
    if !emphasized {
        return theme.muted;
    }
    if idle_seconds < 3 {
        theme.green
    } else if idle_seconds < 60 {
        theme.subtle
    } else if idle_seconds < 3600 {
        theme.muted
    } else {
        theme.dim
    }
}

#[cfg(test)]
#[path = "../../tests/unit/ui/text.rs"]
mod tests;
