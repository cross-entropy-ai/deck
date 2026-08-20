use ratatui::style::Color;

#[derive(Clone, Copy)]
pub struct Theme {
    pub name: &'static str,
    pub bg: Color,
    pub surface: Color,
    /// Raised surfaces such as modals and anchored popovers.
    pub elevated: Color,
    pub dim: Color,
    pub muted: Color,
    pub subtle: Color,
    pub secondary: Color,
    pub text: Color,
    pub accent: Color,
    /// Quiet structural edges and the stronger edge for the active surface.
    pub border: Color,
    pub focus_border: Color,
    /// Full-row selection colors, independent from the decorative accent.
    pub selection_bg: Color,
    pub selection_fg: Color,
    /// Recessed single-line fields and their compact label/border treatment.
    pub input_bg: Color,
    pub input_border: Color,
    /// Scrollbar thumb/track foreground.
    pub scrollbar: Color,
    pub green: Color,
    pub teal: Color,
    pub yellow: Color,
    /// Semantic slot, distinct from the decorative palette so a theme can tune
    /// "this is an error" without recoloring a decorative accent.
    pub error: Color,
}

impl Theme {
    /// Whether this theme reads as dark: Rec. 601 luma of its background, split
    /// at mid-gray. Drives the `CSI ? 997` color-scheme report deck gives the
    /// children attached to its panes.
    pub fn is_dark(&self) -> bool {
        let Color::Rgb(r, g, b) = self.bg else {
            return true;
        };
        0.299 * f64::from(r) + 0.587 * f64::from(g) + 0.114 * f64::from(b) < 128.0
    }
}

/// Materialize the first UI-semantic layer from each palette's existing
/// neutral ramp. Keeping the relationships here gives all themes consistent
/// depth immediately; individual roles can become explicit per-theme values
/// later without changing any renderer call sites.
macro_rules! theme {
    (
        name: $name:expr,
        bg: $bg:expr,
        surface: $surface:expr,
        dim: $dim:expr,
        muted: $muted:expr,
        subtle: $subtle:expr,
        secondary: $secondary:expr,
        text: $text:expr,
        accent: $accent:expr,
        $(selection_bg: $selection_bg:expr,)?
        selection_fg: $selection_fg:expr,
        green: $green:expr,
        teal: $teal:expr,
        yellow: $yellow:expr,
        error: $error:expr $(,)?
    ) => {
        Theme {
            name: $name,
            bg: $bg,
            surface: $surface,
            elevated: $surface,
            dim: $dim,
            muted: $muted,
            subtle: $subtle,
            secondary: $secondary,
            text: $text,
            accent: $accent,
            border: $dim,
            focus_border: $accent,
            selection_bg: theme_selection_bg!($accent $(, $selection_bg)?),
            selection_fg: $selection_fg,
            input_bg: $bg,
            input_border: $dim,
            scrollbar: $muted,
            green: $green,
            teal: $teal,
            yellow: $yellow,
            error: $error,
        }
    };
}

macro_rules! theme_selection_bg {
    ($accent:expr) => {
        $accent
    };
    ($accent:expr, $selection_bg:expr) => {
        $selection_bg
    };
}

pub const THEMES: &[Theme] = &[
    theme! {
        name: "Catppuccin Mocha (Dark)",
        bg: Color::Rgb(30, 30, 46),
        surface: Color::Rgb(49, 50, 68),
        dim: Color::Rgb(88, 91, 112),
        muted: Color::Rgb(108, 112, 134),
        subtle: Color::Rgb(127, 132, 156),
        secondary: Color::Rgb(166, 173, 200),
        text: Color::Rgb(205, 214, 244),
        accent: Color::Rgb(203, 166, 247),
        selection_fg: Color::Rgb(30, 30, 46),
        green: Color::Rgb(166, 227, 161),
        teal: Color::Rgb(148, 226, 213),
        yellow: Color::Rgb(249, 226, 175),
        error: Color::Rgb(243, 139, 168),
    },
    theme! {
        name: "Tokyo Night (Dark)",
        bg: Color::Rgb(26, 27, 38),
        surface: Color::Rgb(36, 40, 59),
        dim: Color::Rgb(65, 72, 104),
        muted: Color::Rgb(86, 95, 137),
        subtle: Color::Rgb(115, 122, 162),
        secondary: Color::Rgb(154, 165, 206),
        text: Color::Rgb(192, 202, 245),
        accent: Color::Rgb(122, 162, 247),
        selection_fg: Color::Rgb(26, 27, 38),
        green: Color::Rgb(158, 206, 106),
        teal: Color::Rgb(115, 218, 202),
        yellow: Color::Rgb(224, 175, 104),
        error: Color::Rgb(187, 154, 247),
    },
    theme! {
        name: "Gruvbox (Dark)",
        bg: Color::Rgb(29, 32, 33),
        surface: Color::Rgb(60, 56, 54),
        dim: Color::Rgb(80, 73, 69),
        muted: Color::Rgb(146, 131, 116),
        subtle: Color::Rgb(168, 153, 132),
        secondary: Color::Rgb(189, 174, 147),
        text: Color::Rgb(235, 219, 178),
        accent: Color::Rgb(131, 165, 152),
        selection_fg: Color::Rgb(29, 32, 33),
        green: Color::Rgb(184, 187, 38),
        teal: Color::Rgb(142, 192, 124),
        yellow: Color::Rgb(250, 189, 47),
        error: Color::Rgb(211, 134, 155),
    },
    theme! {
        name: "Nord (Dark)",
        bg: Color::Rgb(46, 52, 64),
        surface: Color::Rgb(59, 66, 82),
        dim: Color::Rgb(67, 76, 94),
        muted: Color::Rgb(76, 86, 106),
        subtle: Color::Rgb(97, 110, 136),
        secondary: Color::Rgb(216, 222, 233),
        text: Color::Rgb(236, 239, 244),
        accent: Color::Rgb(136, 192, 208),
        selection_fg: Color::Rgb(46, 52, 64),
        green: Color::Rgb(163, 190, 140),
        teal: Color::Rgb(143, 188, 187),
        yellow: Color::Rgb(235, 203, 139),
        error: Color::Rgb(180, 142, 173),
    },
    theme! {
        name: "Dracula (Dark)",
        bg: Color::Rgb(40, 42, 54),
        surface: Color::Rgb(68, 71, 90),
        dim: Color::Rgb(98, 114, 164),
        muted: Color::Rgb(98, 114, 164),
        subtle: Color::Rgb(139, 143, 163),
        secondary: Color::Rgb(248, 248, 242),
        text: Color::Rgb(248, 248, 242),
        accent: Color::Rgb(189, 147, 249),
        selection_fg: Color::Rgb(40, 42, 54),
        green: Color::Rgb(80, 250, 123),
        teal: Color::Rgb(139, 233, 253),
        yellow: Color::Rgb(241, 250, 140),
        error: Color::Rgb(255, 121, 198),
    },
    theme! {
        name: "Claude (Dark)",
        bg: Color::Rgb(26, 26, 26),
        surface: Color::Rgb(46, 38, 32),
        dim: Color::Rgb(90, 72, 56),
        muted: Color::Rgb(112, 100, 90),
        subtle: Color::Rgb(150, 134, 121),
        secondary: Color::Rgb(224, 218, 208),
        text: Color::Rgb(240, 235, 226),
        accent: Color::Rgb(120, 176, 232),
        selection_fg: Color::Rgb(26, 26, 26),
        green: Color::Rgb(152, 200, 122),
        teal: Color::Rgb(94, 196, 208),
        yellow: Color::Rgb(226, 180, 90),
        error: Color::Rgb(212, 144, 208),
    },
    theme! {
        name: "Absolutely (Dark)",
        bg: Color::Rgb(45, 45, 43),
        surface: Color::Rgb(60, 60, 57),
        dim: Color::Rgb(88, 88, 83),
        muted: Color::Rgb(122, 122, 115),
        subtle: Color::Rgb(156, 156, 147),
        secondary: Color::Rgb(207, 207, 198),
        text: Color::Rgb(249, 249, 247),
        accent: Color::Rgb(204, 125, 94),
        selection_fg: Color::Rgb(0, 0, 0),
        green: Color::Rgb(0, 200, 83),
        teal: Color::Rgb(94, 196, 176),
        yellow: Color::Rgb(224, 168, 70),
        error: Color::Rgb(255, 95, 56),
    },
    theme! {
        name: "Codex (Dark)",
        bg: Color::Rgb(17, 17, 17),
        surface: Color::Rgb(33, 33, 33),
        dim: Color::Rgb(64, 64, 64),
        muted: Color::Rgb(110, 110, 110),
        subtle: Color::Rgb(150, 150, 150),
        secondary: Color::Rgb(200, 200, 200),
        text: Color::Rgb(252, 252, 252),
        accent: Color::Rgb(1, 105, 204),
        selection_fg: Color::Rgb(252, 252, 252),
        green: Color::Rgb(0, 162, 64),
        teal: Color::Rgb(41, 182, 216),
        yellow: Color::Rgb(224, 163, 46),
        error: Color::Rgb(224, 46, 42),
    },
    theme! {
        name: "Raycast (Dark)",
        bg: Color::Rgb(16, 16, 16),
        surface: Color::Rgb(32, 32, 32),
        dim: Color::Rgb(62, 62, 62),
        muted: Color::Rgb(108, 108, 108),
        subtle: Color::Rgb(148, 148, 148),
        secondary: Color::Rgb(200, 200, 200),
        text: Color::Rgb(254, 254, 254),
        accent: Color::Rgb(255, 99, 99),
        selection_fg: Color::Rgb(16, 16, 16),
        green: Color::Rgb(89, 212, 153),
        teal: Color::Rgb(79, 200, 192),
        yellow: Color::Rgb(224, 179, 74),
        error: Color::Rgb(207, 47, 152),
    },
    theme! {
        name: "Rose Pine (Dark)",
        bg: Color::Rgb(35, 33, 54),
        surface: Color::Rgb(42, 39, 63),
        dim: Color::Rgb(57, 53, 82),
        muted: Color::Rgb(110, 106, 134),
        subtle: Color::Rgb(144, 140, 170),
        secondary: Color::Rgb(188, 185, 212),
        text: Color::Rgb(224, 222, 244),
        accent: Color::Rgb(234, 154, 151),
        selection_fg: Color::Rgb(35, 33, 54),
        green: Color::Rgb(156, 207, 216),
        teal: Color::Rgb(62, 143, 176),
        yellow: Color::Rgb(246, 193, 119),
        error: Color::Rgb(235, 111, 146),
    },
    theme! {
        name: "GitHub (Dark)",
        bg: Color::Rgb(13, 17, 23),
        surface: Color::Rgb(35, 39, 45),
        dim: Color::Rgb(65, 70, 76),
        muted: Color::Rgb(111, 116, 122),
        subtle: Color::Rgb(143, 149, 155),
        secondary: Color::Rgb(187, 193, 199),
        text: Color::Rgb(230, 237, 243),
        accent: Color::Rgb(31, 111, 235),
        selection_fg: Color::Rgb(255, 255, 255),
        green: Color::Rgb(63, 185, 80),
        teal: Color::Rgb(94, 196, 208),
        yellow: Color::Rgb(224, 175, 104),
        error: Color::Rgb(248, 81, 73),
    },
    theme! {
        name: "Linear (Dark)",
        bg: Color::Rgb(15, 15, 17),
        surface: Color::Rgb(36, 36, 38),
        dim: Color::Rgb(66, 66, 68),
        muted: Color::Rgb(110, 111, 113),
        subtle: Color::Rgb(142, 143, 145),
        secondary: Color::Rgb(185, 185, 187),
        text: Color::Rgb(227, 228, 230),
        accent: Color::Rgb(96, 106, 204),
        selection_fg: Color::Rgb(255, 255, 255),
        green: Color::Rgb(105, 201, 103),
        teal: Color::Rgb(94, 196, 208),
        yellow: Color::Rgb(224, 175, 104),
        error: Color::Rgb(255, 126, 120),
    },
    theme! {
        name: "One (Dark)",
        bg: Color::Rgb(40, 44, 52),
        surface: Color::Rgb(53, 57, 66),
        dim: Color::Rgb(71, 76, 85),
        muted: Color::Rgb(99, 104, 115),
        subtle: Color::Rgb(119, 124, 135),
        secondary: Color::Rgb(145, 151, 163),
        text: Color::Rgb(171, 178, 191),
        accent: Color::Rgb(77, 120, 204),
        selection_fg: Color::Rgb(0, 0, 0),
        green: Color::Rgb(140, 194, 101),
        teal: Color::Rgb(94, 196, 208),
        yellow: Color::Rgb(224, 175, 104),
        error: Color::Rgb(224, 85, 97),
    },
    theme! {
        name: "Vercel (Dark)",
        bg: Color::Rgb(0, 0, 0),
        surface: Color::Rgb(24, 24, 24),
        dim: Color::Rgb(57, 57, 57),
        muted: Color::Rgb(107, 107, 107),
        subtle: Color::Rgb(142, 142, 142),
        secondary: Color::Rgb(190, 190, 190),
        text: Color::Rgb(237, 237, 237),
        accent: Color::Rgb(0, 110, 254),
        selection_fg: Color::Rgb(0, 0, 0),
        green: Color::Rgb(0, 173, 58),
        teal: Color::Rgb(94, 196, 208),
        yellow: Color::Rgb(224, 175, 104),
        error: Color::Rgb(241, 51, 66),
    },
    theme! {
        name: "Catppuccin Latte (Light)",
        bg: Color::Rgb(239, 241, 245),
        surface: Color::Rgb(204, 208, 218),
        dim: Color::Rgb(172, 176, 190),
        muted: Color::Rgb(124, 127, 147),
        subtle: Color::Rgb(108, 111, 133),
        secondary: Color::Rgb(92, 95, 119),
        text: Color::Rgb(76, 79, 105),
        accent: Color::Rgb(136, 57, 239),
        selection_fg: Color::Rgb(239, 241, 245),
        green: Color::Rgb(64, 160, 43),
        teal: Color::Rgb(23, 146, 153),
        yellow: Color::Rgb(156, 110, 16),
        error: Color::Rgb(210, 15, 57),
    },
    theme! {
        name: "Claude (Light)",
        bg: Color::Rgb(249, 245, 239),
        surface: Color::Rgb(233, 224, 212),
        dim: Color::Rgb(201, 184, 160),
        muted: Color::Rgb(150, 134, 121),
        subtle: Color::Rgb(107, 94, 84),
        secondary: Color::Rgb(75, 63, 52),
        text: Color::Rgb(43, 31, 20),
        accent: Color::Rgb(32, 88, 160),
        selection_fg: Color::Rgb(249, 245, 239),
        green: Color::Rgb(61, 117, 32),
        teal: Color::Rgb(16, 112, 112),
        yellow: Color::Rgb(133, 96, 10),
        error: Color::Rgb(136, 62, 149),
    },
    theme! {
        name: "Absolutely (Light)",
        bg: Color::Rgb(249, 249, 247),
        surface: Color::Rgb(236, 236, 231),
        dim: Color::Rgb(206, 206, 198),
        muted: Color::Rgb(150, 150, 141),
        subtle: Color::Rgb(110, 110, 102),
        secondary: Color::Rgb(74, 74, 68),
        text: Color::Rgb(45, 45, 43),
        accent: Color::Rgb(204, 125, 94),
        selection_bg: Color::Rgb(155, 93, 69),
        selection_fg: Color::Rgb(255, 255, 255),
        green: Color::Rgb(0, 200, 83),
        teal: Color::Rgb(47, 144, 128),
        yellow: Color::Rgb(176, 120, 24),
        error: Color::Rgb(255, 95, 56),
    },
    theme! {
        name: "Codex (Light)",
        bg: Color::Rgb(255, 255, 255),
        surface: Color::Rgb(240, 240, 240),
        dim: Color::Rgb(208, 208, 208),
        muted: Color::Rgb(150, 150, 150),
        subtle: Color::Rgb(100, 100, 100),
        secondary: Color::Rgb(60, 60, 60),
        text: Color::Rgb(13, 13, 13),
        accent: Color::Rgb(1, 105, 204),
        selection_fg: Color::Rgb(255, 255, 255),
        green: Color::Rgb(0, 162, 64),
        teal: Color::Rgb(14, 138, 160),
        yellow: Color::Rgb(176, 125, 16),
        error: Color::Rgb(224, 46, 42),
    },
    theme! {
        name: "Raycast (Light)",
        bg: Color::Rgb(255, 255, 255),
        surface: Color::Rgb(240, 240, 240),
        dim: Color::Rgb(208, 208, 208),
        muted: Color::Rgb(148, 148, 148),
        subtle: Color::Rgb(100, 100, 100),
        secondary: Color::Rgb(58, 58, 58),
        text: Color::Rgb(3, 3, 3),
        accent: Color::Rgb(255, 99, 99),
        selection_bg: Color::Rgb(199, 62, 73),
        selection_fg: Color::Rgb(255, 255, 255),
        green: Color::Rgb(0, 107, 79),
        teal: Color::Rgb(14, 138, 150),
        yellow: Color::Rgb(176, 125, 16),
        error: Color::Rgb(154, 27, 110),
    },
    theme! {
        name: "Rose Pine (Light)",
        bg: Color::Rgb(250, 244, 237),
        surface: Color::Rgb(242, 233, 225),
        dim: Color::Rgb(206, 202, 205),
        muted: Color::Rgb(152, 147, 165),
        subtle: Color::Rgb(121, 117, 147),
        secondary: Color::Rgb(104, 99, 134),
        text: Color::Rgb(87, 82, 121),
        accent: Color::Rgb(215, 130, 126),
        selection_bg: Color::Rgb(170, 86, 110),
        selection_fg: Color::Rgb(255, 255, 255),
        green: Color::Rgb(86, 148, 159),
        teal: Color::Rgb(40, 105, 131),
        yellow: Color::Rgb(234, 157, 52),
        error: Color::Rgb(180, 99, 122),
    },
    theme! {
        name: "GitHub (Light)",
        bg: Color::Rgb(255, 255, 255),
        surface: Color::Rgb(242, 242, 242),
        dim: Color::Rgb(219, 220, 221),
        muted: Color::Rgb(161, 163, 165),
        subtle: Color::Rgb(121, 123, 126),
        secondary: Color::Rgb(76, 79, 83),
        text: Color::Rgb(31, 35, 40),
        accent: Color::Rgb(9, 105, 218),
        selection_fg: Color::Rgb(255, 255, 255),
        green: Color::Rgb(26, 127, 55),
        teal: Color::Rgb(14, 130, 150),
        yellow: Color::Rgb(160, 110, 16),
        error: Color::Rgb(207, 34, 46),
    },
    theme! {
        name: "Linear (Light)",
        bg: Color::Rgb(252, 252, 253),
        surface: Color::Rgb(238, 238, 239),
        dim: Color::Rgb(216, 216, 217),
        muted: Color::Rgb(158, 158, 158),
        subtle: Color::Rgb(117, 117, 117),
        secondary: Color::Rgb(72, 72, 72),
        text: Color::Rgb(27, 27, 27),
        accent: Color::Rgb(94, 106, 210),
        selection_fg: Color::Rgb(252, 252, 253),
        green: Color::Rgb(82, 164, 80),
        teal: Color::Rgb(14, 130, 150),
        yellow: Color::Rgb(160, 110, 16),
        error: Color::Rgb(201, 68, 70),
    },
    theme! {
        name: "One (Light)",
        bg: Color::Rgb(250, 250, 250),
        surface: Color::Rgb(238, 238, 239),
        dim: Color::Rgb(219, 219, 221),
        muted: Color::Rgb(169, 169, 173),
        subtle: Color::Rgb(134, 135, 140),
        secondary: Color::Rgb(95, 96, 103),
        text: Color::Rgb(56, 58, 66),
        accent: Color::Rgb(82, 111, 255),
        selection_bg: Color::Rgb(69, 99, 230),
        selection_fg: Color::Rgb(255, 255, 255),
        green: Color::Rgb(59, 186, 84),
        teal: Color::Rgb(14, 130, 150),
        yellow: Color::Rgb(160, 110, 16),
        error: Color::Rgb(228, 86, 73),
    },
    theme! {
        name: "Proof (Light)",
        bg: Color::Rgb(245, 243, 237),
        surface: Color::Rgb(233, 231, 225),
        dim: Color::Rgb(213, 212, 206),
        muted: Color::Rgb(162, 162, 156),
        subtle: Color::Rgb(126, 127, 122),
        secondary: Color::Rgb(87, 88, 83),
        text: Color::Rgb(47, 49, 45),
        accent: Color::Rgb(61, 117, 93),
        selection_fg: Color::Rgb(245, 243, 237),
        green: Color::Rgb(61, 117, 93),
        teal: Color::Rgb(14, 130, 150),
        yellow: Color::Rgb(160, 110, 16),
        error: Color::Rgb(186, 38, 35),
    },
    theme! {
        name: "Vercel (Light)",
        bg: Color::Rgb(255, 255, 255),
        surface: Color::Rgb(241, 241, 241),
        dim: Color::Rgb(218, 218, 218),
        muted: Color::Rgb(158, 158, 158),
        subtle: Color::Rgb(116, 116, 116),
        secondary: Color::Rgb(69, 69, 69),
        text: Color::Rgb(23, 23, 23),
        accent: Color::Rgb(0, 106, 255),
        selection_fg: Color::Rgb(255, 255, 255),
        green: Color::Rgb(40, 169, 72),
        teal: Color::Rgb(14, 130, 150),
        yellow: Color::Rgb(160, 110, 16),
        error: Color::Rgb(235, 0, 29),
    },
];

/// Which theme slot a picker edits: the fixed choice, or one of the two
/// "follow terminal" slots. See `Prefs::theme_auto`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ThemeSlot {
    #[default]
    Fixed,
    Dark,
    Light,
}

/// Global theme indices available to a picker for `slot`. The fixed-theme
/// picker can choose any theme, while Auto theme's dark/light slots only show
/// palettes matching the appearance they will be used for.
pub fn indices_for_slot(slot: ThemeSlot) -> impl Iterator<Item = usize> {
    THEMES
        .iter()
        .enumerate()
        .filter(move |(_, theme)| match slot {
            ThemeSlot::Fixed => true,
            ThemeSlot::Dark => theme.is_dark(),
            ThemeSlot::Light => !theme.is_dark(),
        })
        .map(|(index, _)| index)
}

/// Index of the theme named `name`, falling back to the first theme (the
/// default) when a config names one that no longer exists.
pub fn index_of(name: &str) -> usize {
    THEMES.iter().position(|t| t.name == name).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colored_light_theme_selections_use_white_text() {
        for name in [
            "Absolutely (Light)",
            "Raycast (Light)",
            "Rose Pine (Light)",
            "One (Light)",
        ] {
            assert_eq!(
                THEMES[index_of(name)].selection_fg,
                Color::Rgb(255, 255, 255),
                "{name}"
            );
        }
    }
}
