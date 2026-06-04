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
use crate::ui::{self, PluginStatus, PluginView, SettingsView};
use crate::update::UpdateCheckMode;

use super::update::format_update_check_help;
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
        let theme = &THEMES[s.theme_index];
        let show_help = s.overlay.show_help;
        let rename_input = s.overlay.renaming.as_ref().map(|r| &r.input);
        let context_menu = s.overlay.context_menu.clone();
        let new_session_overlay = s.overlay.new_session.clone();
        let add_remote_overlay = s.overlay.add_remote.clone();
        let port_forward_overlay = s.overlay.port_forward.clone();
        let show_borders = s.show_borders;
        let show_agents = s.show_agents;
        let layout_mode = s.layout_mode;
        let view_mode = s.view_mode;
        let sidebar_width = s.sidebar_width;
        let sidebar_height = s.effective_sidebar_height();
        let main_view = s.main_view;
        let warning_state = self.warning_state.clone();

        let confirm_name = s.confirm_kill_name();

        let update_available = s.update_available.clone();
        let reload_status = s.reload_status.clone();
        let dragging_sep = s.dragging_separator;

        let mut captured_banner_bounds: Option<Rect> = None;
        let mut captured_divider_hits: Vec<crate::state::DividerHit> = Vec::new();
        let mut captured_kill_hits: Option<crate::state::KillConfirmHits> = None;
        let mut captured_agent_hits: Vec<crate::state::AgentHit> = Vec::new();
        let mut captured_agents_checkbox: Option<Rect> = None;
        terminal.draw(|frame| {
            // Unified slice the sidebar consumes: local rows first
            // (flat index == filtered_pos), then remotes (flat index
            // == local_count + remote_idx). Both SessionRow and
            // RemoteSessionRow impl SidebarSession directly, so the
            // sidebar reads straight from storage — no per-frame
            // borrowed-view shells needed.
            let local_count = self.state.filtered.len();
            let sessions_dyn: Vec<&dyn ui::SidebarSession> = self
                .state
                .filtered
                .iter()
                .map(|&i| &self.state.sessions[i] as &dyn ui::SidebarSession)
                .chain(
                    self.state
                        .remote_sessions
                        .iter()
                        .map(|r| r as &dyn ui::SidebarSession),
                )
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

            let layout = self.state.sidebar_layout(view_mode);
            let focus_target = self.state.focus_target();
            let (banner_bounds, divider_hits, kill_hits, agent_hits, agents_checkbox) =
                ui::draw_sidebar(
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
                        show_agents,
                        tabs_mode: layout_mode == LayoutMode::Vertical,
                        view_mode,
                        plugins: &plugin_views,
                        blink_on,
                        keybindings: &self.state.keybindings,
                        update_available: update_available.as_ref(),
                        active_agent: self.state.active_agent.as_ref(),
                    },
                );
            captured_banner_bounds = banner_bounds;
            captured_divider_hits = divider_hits;
            captured_kill_hits = kill_hits;
            captured_agent_hits = agent_hits;
            captured_agents_checkbox = agents_checkbox;

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
            let local_active_dead = self.active_remote.is_none() && !self.local_terminal.alive;
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
            let background_screen = match (warning_state.as_ref(), main_view) {
                (
                    Some(crate::state::WarningState::Proactive { .. }),
                    _,
                ) => None,
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

            // Built only when the Settings page is actually showing —
            // it allocates the update-check help string, which would
            // otherwise be thrown away every other frame.
            if warning_state.is_none() && main_view == MainView::Settings {
                let settings_view = SettingsView {
                    selected: s.settings.selected,
                    theme_name: THEMES[s.theme_index].name,
                    layout_mode: s.layout_mode,
                    show_borders: s.show_borders,
                    view_mode: s.view_mode,
                    exclude_count: s.exclude_patterns.len(),
                    exclude_editor: s.overlay.exclude_editor.as_ref().map(|e| {
                        ui::ExcludeEditorView {
                            patterns: &s.exclude_patterns,
                            selected: e.selected,
                            adding: e.adding,
                            input: &e.input,
                            error: e.error.as_deref(),
                        }
                    }),
                    keybindings: &s.keybindings,
                    keybindings_view_open: s.settings.keybindings_view_open,
                    keybindings_view_scroll: s.settings.keybindings_view_scroll,
                    update_check_enabled: s.update_check_mode == UpdateCheckMode::Enabled,
                    update_check_help: format_update_check_help(s.update_last_checked_secs),
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
                            text.to_string(),
                            detail,
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

            if let Some(ref menu) = context_menu {
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

            if let Some(ref ns) = new_session_overlay {
                let view = ui::NewSessionView {
                    name: &ns.name,
                    focus_name: matches!(ns.focus, crate::new_session::PickerFocus::Name),
                    input: &ns.input,
                    entries: &ns.entries,
                    filtered: &ns.filtered,
                    selected: ns.selected,
                    error: ns.error.as_deref(),
                    host: ns.remote_host.as_deref(),
                };
                ui::draw_new_session(frame, frame.area(), &view, theme);
            }

            if let Some(ref ar) = add_remote_overlay {
                ui::draw_add_remote(frame, frame.area(), ar, theme);
            }

            if let Some(ref overlay) = port_forward_overlay {
                let pf_area = frame.area();
                crate::ui::overlays::port_forward::draw_port_forward(
                    frame.buffer_mut(),
                    pf_area,
                    overlay,
                    &self.state.config_remotes,
                    &self.state.forward_health,
                    theme,
                );
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
        self.state.divider_hits = captured_divider_hits;
        self.state.kill_confirm_hits = captured_kill_hits;
        self.state.agent_hits = captured_agent_hits;
        self.state.show_agents_checkbox = captured_agents_checkbox;

        Ok(())
    }
}
