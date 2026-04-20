use std::io;
use std::time::{SystemTime, UNIX_EPOCH};

use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::DefaultTerminal;

use crate::bridge;
use crate::state::{FocusMode, LayoutMode, MainView, SessionRow};
use crate::theme::THEMES;
use crate::ui::{self, PluginStatus, PluginView, SessionView, SettingsView};
use crate::update::UpdateCheckMode;

use super::update::format_update_check_help;
use super::App;

impl App {
    pub(super) fn render(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        let s = &self.state;
        let sidebar_active = s.focus_mode == FocusMode::Sidebar;
        let focused = s.focused;
        let theme = &THEMES[s.theme_index];
        let confirm_kill = s.confirm_kill;
        let show_help = s.show_help;
        let rename_input = s.renaming.as_ref().map(|r| (r.input.clone(), r.cursor));
        let context_menu = s.context_menu.clone();
        let show_borders = s.show_borders;
        let layout_mode = s.layout_mode;
        let view_mode = s.view_mode;
        let sidebar_width = s.sidebar_width;
        let sidebar_height = s.effective_sidebar_height();
        let main_view = s.main_view;
        let warning_state = self.warning_state.clone();

        let confirm_name = if confirm_kill {
            s.filtered
                .get(s.focused)
                .map(|&i| s.sessions[i].name.clone())
        } else {
            None
        };

        let views_owned: Vec<(SessionRow, crate::state::SessionStatus)> = s
            .filtered
            .iter()
            .map(|&i| {
                let row = &s.sessions[i];
                (row.clone(), s.effective_status(row))
            })
            .collect();

        let spinner_frame = self.spinner.current_frame().to_string();
        let update_check_help = format_update_check_help(s.update_last_checked_secs);
        let update_check_mode = s.update_check_mode;
        let settings_view = SettingsView {
            selected: s.settings_selected,
            focus_main: s.focus_mode == FocusMode::Main,
            theme_name: THEMES[s.theme_index].name,
            theme_picker_open: s.theme_picker_open,
            theme_picker_selected: s.theme_picker_selected,
            theme_names: THEMES.iter().map(|theme| theme.name).collect(),
            layout_mode: s.layout_mode,
            show_borders: s.show_borders,
            view_mode: s.view_mode,
            exclude_count: s.exclude_patterns.len(),
            exclude_editor: s.exclude_editor.as_ref().map(|e| ui::ExcludeEditorView {
                patterns: &s.exclude_patterns,
                selected: e.selected,
                adding: e.adding,
                input: &e.input,
                error: e.error.as_deref(),
            }),
            keybindings: &s.keybindings,
            keybindings_view_open: s.keybindings_view_open,
            keybindings_view_scroll: s.keybindings_view_scroll,
            update_check_enabled: update_check_mode == UpdateCheckMode::Enabled,
            update_check_help,
        };
        let update_available = s.update_available.clone();
        let reload_status = s.reload_status.clone();
        let hover_sep = s.hover_separator;
        let dragging_sep = s.dragging_separator;

        let mut captured_banner_bounds: Option<Rect> = None;
        terminal.draw(|frame| {
            let views: Vec<SessionView> = views_owned
                .iter()
                .map(|(r, status)| SessionView {
                    name: r.name.as_str(),
                    dir: r.dir.as_str(),
                    branch: r.branch.as_str(),
                    ahead: r.ahead,
                    behind: r.behind,
                    staged: r.staged,
                    modified: r.modified,
                    untracked: r.untracked,
                    idle_seconds: r.idle_seconds,
                    status: *status,
                    is_current: r.is_current,
                })
                .collect();

            let full = frame.area();
            let reload_height = ui::reload_row_count(reload_status.as_ref(), full.width);
            // Paint the reload bar as an overlay after everything else,
            // not as its own layout slot. Keeping the content area at
            // full height means PTY sizing (see `AppState::pty_size`)
            // and mouse routing stay stable when the bar pops in.
            let reload_area = if reload_height > 0 {
                Some(Rect {
                    x: full.x,
                    y: full.bottom().saturating_sub(reload_height),
                    width: full.width,
                    height: reload_height,
                })
            } else {
                None
            };

            let (sidebar_area, gap_area, main_area) = match layout_mode {
                LayoutMode::Horizontal => {
                    let [s, g, m] = Layout::horizontal([
                        Constraint::Length(sidebar_width),
                        Constraint::Length(1),
                        Constraint::Min(1),
                    ])
                    .areas(full);
                    (s, Some(g), m)
                }
                LayoutMode::Vertical => {
                    let [s, m] =
                        Layout::vertical([Constraint::Length(sidebar_height), Constraint::Min(1)])
                            .areas(full);
                    (s, None, m)
                }
            };

            let plugin_views: Vec<PluginView> = self
                .state
                .plugins
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    let alive = self
                        .plugin_instances
                        .get(i)
                        .and_then(|slot| slot.as_ref())
                        .map(|inst| inst.alive)
                        .unwrap_or(false);
                    let status = match (alive, main_view == MainView::Plugin(i)) {
                        (true, true) => PluginStatus::Foreground,
                        (true, false) => PluginStatus::Background,
                        (false, _) => PluginStatus::Inactive,
                    };
                    PluginView {
                        key: p.key,
                        name: p.name.as_str(),
                        status,
                    }
                })
                .collect();

            // 1 Hz pulse for plugins running in the background — the main
            // loop already redraws every ~16 ms so we don't need a tick.
            let blink_on = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| (d.as_millis() / 500) % 2 == 0)
                .unwrap_or(true);

            captured_banner_bounds = ui::draw_sidebar(
                frame,
                sidebar_area,
                &views,
                focused,
                sidebar_active,
                theme,
                show_help,
                confirm_name.as_deref(),
                rename_input.as_ref().map(|(s, c)| (s.as_str(), *c)),
                show_borders,
                layout_mode == LayoutMode::Vertical,
                &spinner_frame,
                view_mode,
                &plugin_views,
                blink_on,
                &self.state.keybindings,
                update_available.as_ref(),
            );

            if let Some(gap) = gap_area {
                let (sep_char, sep_fg) = if dragging_sep {
                    ('┃', theme.green)
                } else if hover_sep {
                    ('┃', theme.accent)
                } else {
                    ('│', theme.dim)
                };
                for y in gap.y..gap.bottom() {
                    if let Some(cell) = frame.buffer_mut().cell_mut((gap.x, y)) {
                        cell.set_char(sep_char);
                        cell.set_style(ratatui::style::Style::default().fg(sep_fg).bg(theme.bg));
                    }
                }
            }

            let screen = self.parser.screen();
            let plugin_screen = match main_view {
                MainView::Plugin(idx) => self
                    .plugin_instances
                    .get(idx)
                    .and_then(|o| o.as_ref())
                    .map(|inst| inst.parser.screen()),
                _ => None,
            };
            let upgrade_screen = match main_view {
                MainView::Upgrade => self
                    .upgrade_instance
                    .as_ref()
                    .map(|inst| inst.parser.screen()),
                _ => None,
            };
            let background_screen = match (warning_state.as_ref(), main_view) {
                (
                    Some(
                        crate::nesting_guard::WarningState::Proactive { .. }
                        | crate::nesting_guard::WarningState::Detected(_),
                    ),
                    _,
                ) => None,
                (None, MainView::Terminal) => Some(screen),
                (None, MainView::Plugin(_)) => plugin_screen,
                (None, MainView::Upgrade) => upgrade_screen,
                (None, MainView::Settings) => None,
            };

            let main_base = Style::default().fg(theme.text).bg(theme.bg);

            let main_inner = if show_borders {
                let main_border_color = if sidebar_active {
                    theme.dim
                } else {
                    theme.accent
                };
                let main_block = Block::default()
                    .borders(Borders::ALL)
                    .border_set(ratatui::symbols::border::ROUNDED)
                    .border_style(Style::default().fg(main_border_color))
                    .style(main_base);
                let main_inner = main_block.inner(main_area);
                frame.render_widget(main_block, main_area);
                if let Some(screen) = background_screen {
                    bridge::render_screen(
                        screen,
                        main_inner,
                        frame.buffer_mut(),
                        theme.text,
                        theme.bg,
                    );
                    if !sidebar_active && warning_state.is_none() {
                        bridge::set_cursor(frame, screen, main_inner);
                    }
                }
                main_inner
            } else {
                frame.render_widget(Block::default().style(main_base), main_area);
                if let Some(screen) = background_screen {
                    bridge::render_screen(
                        screen,
                        main_area,
                        frame.buffer_mut(),
                        theme.text,
                        theme.bg,
                    );
                    if !sidebar_active && warning_state.is_none() {
                        bridge::set_cursor(frame, screen, main_area);
                    }
                }
                main_area
            };

            if warning_state.is_none() && main_view == MainView::Settings {
                ui::draw_settings_page(frame, main_inner, &settings_view, theme);
            }

            if let Some(warning_state) = warning_state {
                let (title, border_color, main_style, sub_style, warning_text, detail_text) =
                    match warning_state {
                        crate::nesting_guard::WarningState::Proactive { text, detail } => (
                            " Heads up ",
                            theme.yellow,
                            Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
                            Style::default().fg(theme.dim),
                            text.to_string(),
                            detail,
                        ),
                        crate::nesting_guard::WarningState::Detected(text) => (
                            " Warning ",
                            theme.pink,
                            Style::default().fg(theme.pink).add_modifier(Modifier::BOLD),
                            Style::default().fg(theme.dim),
                            text.to_string(),
                            "This session now contains deck.\nSwitch away from it in the sidebar."
                                .to_string(),
                        ),
                    };

                let warning = Paragraph::new(vec![
                    Line::from(Span::styled(warning_text, main_style)),
                    Line::raw(""),
                    Line::from(Span::styled(detail_text, sub_style)),
                ])
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_set(ratatui::symbols::border::ROUNDED)
                        .title(title)
                        .border_style(Style::default().fg(border_color)),
                )
                .alignment(Alignment::Center)
                .wrap(Wrap { trim: true });

                frame.render_widget(
                    Block::default().style(Style::default().bg(theme.bg)),
                    main_inner,
                );
                let popup_area = Self::centered_rect(main_inner, 56, 8);
                frame.render_widget(Clear, popup_area);
                frame.render_widget(warning, popup_area);
            }

            if let Some(ref menu) = context_menu {
                ui::draw_context_menu(frame, menu.x, menu.y, menu.selected, menu.items(), theme);
            }

            // Overlay the reload bar last so it sits on top of the sidebar
            // footer, main pane, warning popup, and context menu. The
            // underlying layouts keep their full area, so PTY sizing and
            // mouse routing are unaffected by the bar's presence.
            if let (Some(status), Some(area)) = (reload_status.as_ref(), reload_area) {
                frame.render_widget(Clear, area);
                ui::draw_reload_bar(frame, area, status, theme);
            }
        })?;

        self.state.banner_upgrade_bounds = captured_banner_bounds;

        Ok(())
    }
}
