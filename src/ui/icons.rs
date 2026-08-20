//! Semantic icons with an explicit compatibility ladder.
//!
//! Terminal applications cannot reliably detect whether the configured font
//! contains Nerd Font private-use glyphs. Deck therefore defaults to ordinary
//! Unicode, offers a strict ASCII fallback, and keeps Nerd Font glyphs as an
//! opt-in through `DECK_ICON_STYLE=nerd`.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Icon {
    Sessions,
    Agents,
    Keyboard,
    Mouse,
    Summary,
    Open,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum IconStyle {
    #[default]
    Unicode,
    Ascii,
    Nerd,
}

fn icon_style(value: Option<&str>) -> IconStyle {
    match value.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
        Some("ascii") => IconStyle::Ascii,
        Some("nerd") | Some("nerd-font") => IconStyle::Nerd,
        Some("unicode") | None => IconStyle::Unicode,
        Some(_) => IconStyle::Unicode,
    }
}

fn selected_style() -> IconStyle {
    icon_style(std::env::var("DECK_ICON_STYLE").ok().as_deref())
}

fn glyph_for(icon: Icon, style: IconStyle) -> &'static str {
    match (icon, style) {
        (Icon::Sessions, IconStyle::Unicode) => "▤",
        (Icon::Agents, IconStyle::Unicode) => "⚙",
        (Icon::Keyboard, IconStyle::Unicode) => "⌨",
        (Icon::Mouse, IconStyle::Unicode) => "◉",
        (Icon::Summary, IconStyle::Unicode) => "✦",
        (Icon::Open, IconStyle::Unicode) => "↗",
        (Icon::Sessions, IconStyle::Ascii) => "#",
        (Icon::Agents, IconStyle::Ascii) => "@",
        (Icon::Keyboard, IconStyle::Ascii) => "K",
        (Icon::Mouse, IconStyle::Ascii) => "M",
        (Icon::Summary, IconStyle::Ascii) => "*",
        (Icon::Open, IconStyle::Ascii) => "^",
        (Icon::Sessions, IconStyle::Nerd) => "\u{e795}",
        (Icon::Agents, IconStyle::Nerd) => "\u{f085}",
        (Icon::Keyboard, IconStyle::Nerd) => "\u{f030c}",
        (Icon::Mouse, IconStyle::Nerd) => "\u{f037d}",
        (Icon::Summary, IconStyle::Nerd) => "\u{f0eb}",
        (Icon::Open, IconStyle::Nerd) => "\u{f065}",
    }
}

pub(super) fn icon(icon: Icon) -> &'static str {
    glyph_for(icon, selected_style())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Icon; 6] = [
        Icon::Sessions,
        Icon::Agents,
        Icon::Keyboard,
        Icon::Mouse,
        Icon::Summary,
        Icon::Open,
    ];

    #[test]
    fn unicode_is_the_default_and_unknown_values_fall_back_to_it() {
        assert_eq!(icon_style(None), IconStyle::Unicode);
        assert_eq!(icon_style(Some("unicode")), IconStyle::Unicode);
        assert_eq!(icon_style(Some("future-style")), IconStyle::Unicode);
    }

    #[test]
    fn fallback_styles_never_emit_private_use_glyphs() {
        for style in [IconStyle::Unicode, IconStyle::Ascii] {
            for icon in ALL {
                assert!(glyph_for(icon, style)
                    .chars()
                    .all(|c| !(('\u{e000}'..='\u{f8ff}').contains(&c))));
            }
        }
    }

    #[test]
    fn ascii_style_is_single_cell_ascii() {
        for icon in ALL {
            let glyph = glyph_for(icon, IconStyle::Ascii);
            assert!(glyph.is_ascii());
            assert_eq!(glyph.len(), 1);
        }
    }
}
