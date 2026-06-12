use std::io;
use std::time::{SystemTime, UNIX_EPOCH};

use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::DefaultTerminal;

use crate::bridge;
use crate::state::{FocusMode, LayoutMode, MainView};
use crate::theme::THEMES;
use crate::ui::{self, PluginStatus, PluginView, SettingRowView, SettingsView};

use super::settings::SETTING_ROWS;
use super::App;

impl App {
    pub(super) fn render(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        if self.needs_full_redraw {
            // On a session switch the host terminal emulator can leave
            // stale characters from the previous session on screen.
            // `terminal.clear()` issues an ANSI clear-screen to the
            // host terminal AND resets ratatui's previous-frame buffer,
            // forcing the next draw to emit every cell — a clean repaint
            // that wipes any residue.
            terminal.clear()?;
            self.needs_full_redraw = false;
        }
        let s = &self.state;
        let sidebar_active = s.focus_mode == FocusMode::Sidebar;
        let base_theme = THEMES[s.prefs.theme_index];
        let effective_theme;
        let theme = if s.prefs.transparent_bg {
            effective_theme = crate::theme::Theme {
                bg: ratatui::style::Color::Reset,
                ..base_theme
            };
            &effective_theme
        } else {
            &base_theme
        };
        let show_help = s.overlay.show_help;
        let rename_input = s.overlay.renaming.as_ref().map(|r| &r.input);
        // Overlay state is only *read* inside the draw closure, so borrow
        // it — cloning TextAreas/entry lists/summary text every frame was
        // pure allocation churn.
        let context_menu = s.overlay.context_menu.as_ref();
        // The summary popup shows the Ready text in a big centered view.
        let summary_popup = match (&s.summary.state, s.overlay.summary_popup) {
            (crate::state::SummaryState::Ready { text, .. }, true) => Some(text.as_str()),
            _ => None,
        };
        let summary_popup_scroll = s.summary.popup_scroll;
        let new_session_overlay = s.overlay.new_session.as_ref();
        let add_remote_overlay = s.overlay.add_remote.as_ref();
        let port_forward_overlay = s.overlay.port_forward.as_ref();
        let show_borders = s.prefs.show_borders;
        let sidebar_tab = s.prefs.sidebar_tab;
        let layout_mode = s.prefs.layout_mode;
        let view_mode = s.prefs.view_mode;
        let sidebar_width = s.prefs.sidebar_width;
        let sidebar_height = s.effective_sidebar_height();
        let main_view = s.main_view;
        let warning_state = self.warning_state.as_ref();
        let remote_placeholder = s.focused_remote_placeholder().map(|entry| {
            let host = entry.host.as_deref().unwrap_or_default();
            let (title, detail) = match entry.kind {
                crate::state::SessionKind::Connecting => (
                    format!("Connecting to @{host}"),
                    "Waiting for the remote terminal to connect".to_string(),
                ),
                crate::state::SessionKind::Unreachable => (
                    format!("Cannot reach @{host}"),
                    "Reconnect this host from the sidebar".to_string(),
                ),
                crate::state::SessionKind::NoSessions => (
                    format!("No sessions for @{host}"),
                    "Create one from the host menu to attach here".to_string(),
                ),
                // A focused remote placeholder is never `Live`, but keep a
                // sensible fallback string rather than panic.
                crate::state::SessionKind::Live { .. } => (
                    format!("No attachable session for @{host}"),
                    "Create one from the host menu to attach here".to_string(),
                ),
            };
            (title, detail)
        });

        let confirm_name = s.confirm_kill_name();

        let update_available = s.update_available.as_ref();
        let reload_status = s.reload_status.as_ref();
        let dragging_sep = s.dragging_separator;

        let mut captured_hits = crate::state::HitRegions::default();
        let mut captured_summary_popup_max_scroll: usize = 0;
        terminal.draw(|frame| {
            // Unified slice the sidebar consumes: `entries` is already in
            // render/flat order (local rows first, then remotes), and
            // `SessionEntry` impls `SidebarSession`, so the sidebar reads
            // straight from storage — no per-frame borrowed-view shells.
            let local_count = self.state.local_count();
            let sessions_dyn: Vec<&dyn ui::SidebarSession> = self
                .state
                .entries
                .iter()
                .map(|e| e as &dyn ui::SidebarSession)
                .collect();

            let full = frame.area();
            let reload_height = ui::reload_row_count(reload_status, full.width);
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
                .prefs
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
            // ~12.5 fps braille spinner for the Summary card; sessions.rs
            // takes this mod the frame count.
            let spinner_idx = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| (d.as_millis() / 80) as usize)
                .unwrap_or(0);

            // The Summary card shows how long ago its text landed; compute the
            // "Xm ago" age here so the renderer stays free of wall-clock reads.
            let summary_age = match &self.state.summary.state {
                crate::state::SummaryState::Ready { generated_at, .. } => Some(
                    crate::update::relative_age(
                        crate::update::now_secs().saturating_sub(*generated_at),
                    ),
                ),
                _ => None,
            };

            let layout = self.state.current_layout(view_mode);
            let agent_rows = self.state.agent_rows();
            let focus_target = self.state.focus_target();
            captured_hits = ui::draw_sidebar(
                frame,
                sidebar_area,
                ui::SidebarProps {
                    sessions: &sessions_dyn,
                    local_count,
                    layout: &layout,
                    focus_target,
                    sidebar_active,
                    theme,
                    show_help,
                    confirm_kill: confirm_name.as_deref(),
                    rename_input,
                    show_borders,
                    sidebar_tab,
                    agent_rows: &agent_rows,
                    summary: &self.state.summary.state,
                    summary_age: summary_age.as_deref(),
                    spinner_idx,
                    summary_scroll: self.state.summary.scroll,
                    tabs_mode: layout_mode == LayoutMode::Vertical,
                    view_mode,
                    plugins: &plugin_views,
                    blink_on,
                    keybindings: &self.state.keybindings,
                    update_available,
                    active_agent: self.state.active_agent.as_ref(),
                },
            );

            if let Some(gap) = gap_area {
                let (sep_char, sep_fg) = if dragging_sep {
                    ('┃', theme.green)
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

            // The local attach PTY is dead and it's the active view (no
            // remote selected): there are no local sessions to show, so we
            // render an empty-state placeholder instead of a stale screen.
            let local_active_dead = self.remote.active().is_none() && !self.local_terminal.alive;
            let screen = self.active_terminal().parser.screen();
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
            let background_screen = match (warning_state, main_view) {
                (Some(crate::state::WarningState::Proactive { .. }), _) => None,
                (None, MainView::Terminal) if remote_placeholder.is_some() => None,
                // Dead local pane (no sessions to attach to) renders the
                // empty-state placeholder below instead of a stale screen.
                (None, MainView::Terminal) if local_active_dead => None,
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

            // Empty-state placeholder for a dead local pane (no sessions to
            // attach to). deck stays open instead of quitting; the user can
            // create a session from the sidebar (or `q` to quit).
            if warning_state.is_none() && main_view == MainView::Terminal && local_active_dead {
                let lines = vec![
                    Line::from(""),
                    Line::from(Span::styled(
                        "No local sessions",
                        Style::default().fg(theme.text),
                    )),
                    Line::from(Span::styled(
                        "Create one from the sidebar to attach here",
                        Style::default().fg(theme.dim),
                    )),
                ];
                let placeholder = Paragraph::new(lines)
                    .alignment(Alignment::Center)
                    .wrap(Wrap { trim: true });
                frame.render_widget(placeholder, main_inner);
            }

            if warning_state.is_none() && main_view == MainView::Terminal {
                if let Some((title, detail)) = remote_placeholder.as_ref() {
                    let lines = vec![
                        Line::from(""),
                        Line::from(Span::styled(
                            title.as_str(),
                            Style::default().fg(theme.text),
                        )),
                        Line::from(Span::styled(
                            detail.as_str(),
                            Style::default().fg(theme.dim),
                        )),
                    ];
                    let placeholder = Paragraph::new(lines)
                        .alignment(Alignment::Center)
                        .wrap(Wrap { trim: true });
                    frame.render_widget(placeholder, main_inner);
                }
            }

            // Built only when the Settings page is actually showing —
            // it allocates the update-check help string, which would
            // otherwise be thrown away every other frame.
            if warning_state.is_none() && main_view == MainView::Settings {
                // Reduce the descriptor table to display strings here, where
                // we still hold `&AppState`: the value/help closures read it,
                // but `draw_settings_page` is a pure `ui` fn that only sees
                // the resulting `Vec<SettingRowView>`.
                let rows: Vec<SettingRowView> = SETTING_ROWS
                    .iter()
                    .map(|row| SettingRowView {
                        label: row.label,
                        value: (row.value)(s),
                        help: (row.help)(s),
                    })
                    .collect();
                let settings_view = SettingsView {
                    selected: s.settings.selected,
                    rows,
                    exclude_editor: s.overlay.exclude_editor.as_ref().map(|e| {
                        ui::ExcludeEditorView {
                            patterns: &s.prefs.exclude_patterns,
                            selected: e.selected,
                            adding: e.adding,
                            input: &e.input,
                            error: e.error.as_deref(),
                        }
                    }),
                    keybindings: &s.keybindings,
                    keybindings_view_open: s.settings.keybindings_view_open,
                    keybindings_view_scroll: s.settings.keybindings_view_scroll,
                    summary_lang_input: s.overlay.summary_lang_input.as_ref(),
                };
                ui::draw_settings_page(frame, main_inner, &settings_view, theme);
            }

            // Theme picker — a standalone overlay drawn over the main pane
            // whenever it's open, on top of the settings page if that's
            // showing or directly over the terminal when opened from the
            // sidebar (`t`). Decoupled from the settings page so it can
            // bypass it entirely.
            if warning_state.is_none() && s.settings.theme_picker_open {
                let theme_names: Vec<&str> = THEMES.iter().map(|t| t.name).collect();
                ui::draw_theme_picker(
                    frame,
                    main_inner,
                    &theme_names,
                    s.settings.theme_picker_selected,
                    theme,
                );
            }

            if let Some(warning_state) = warning_state {
                let (title, border_color, main_style, sub_style, warning_text, detail_text) =
                    match warning_state {
                        crate::state::WarningState::Proactive { text, detail } => (
                            " Heads up ",
                            theme.yellow,
                            Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
                            Style::default().fg(theme.dim),
                            *text,
                            detail.as_str(),
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
                let popup_area = crate::ui::widgets::centered_rect(main_inner, 56, 8);
                frame.render_widget(Clear, popup_area);
                frame.render_widget(warning, popup_area);
            }

            if let Some(menu) = context_menu {
                ui::draw_context_menu(
                    frame,
                    menu.x,
                    menu.y,
                    menu.selected,
                    menu.items(),
                    menu.disabled(),
                    theme,
                );
            }

            if let Some(ns) = new_session_overlay {
                let view = ui::NewSessionView {
                    name: &ns.name,
                    focus_name: matches!(ns.focus, crate::new_session::PickerFocus::Name),
                    input: &ns.picker.input,
                    entries: &ns.picker.items,
                    filtered: &ns.picker.filtered,
                    selected: ns.picker.selected,
                    error: ns.picker.error.as_deref(),
                    host: ns.remote_host.as_deref(),
                };
                ui::draw_new_session(frame, frame.area(), &view, theme);
            }

            if let Some(ar) = add_remote_overlay {
                ui::draw_add_remote(frame, frame.area(), ar, theme);
            }

            if let Some(overlay) = port_forward_overlay {
                let pf_area = frame.area();
                crate::ui::overlays::port_forward::draw_port_forward(
                    frame,
                    pf_area,
                    overlay,
                    &self.state.config_remotes,
                    &self.state.forward_health,
                    theme,
                );
            }

            if let Some(text) = summary_popup {
                captured_summary_popup_max_scroll = ui::draw_summary_popup(
                    frame,
                    frame.area(),
                    text,
                    summary_popup_scroll,
                    theme,
                );
            }

            // Overlay the reload bar last so it sits on top of the sidebar
            // footer, main pane, warning popup, and context menu. The
            // underlying layouts keep their full area, so PTY sizing and
            // mouse routing are unaffected by the bar's presence.
            if let (Some(status), Some(area)) = (reload_status, reload_area) {
                frame.render_widget(Clear, area);
                ui::draw_reload_bar(frame, area, status, theme);
            }
        })?;

        self.state.hit_regions = captured_hits;
        self.state.summary.popup_max_scroll = captured_summary_popup_max_scroll;

        Ok(())
    }
}
