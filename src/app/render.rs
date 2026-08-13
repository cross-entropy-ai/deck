use std::io;
use std::time::{SystemTime, UNIX_EPOCH};

use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};

use crate::bridge;
use crate::overlay::Modal;
use crate::state::{FocusMode, LayoutMode, MainView};
use crate::ui::{self, SettingRowView, SettingsView};

use super::modal::draw_active_modal;
use super::settings::setting_rows;
use super::App;

impl App {
    pub(super) fn render(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        if self.needs_full_redraw {
            // A session switch can leave stale characters on screen.
            // `terminal.clear()` issues an ANSI clear-screen AND resets
            // ratatui's previous-frame buffer, forcing the next draw to emit
            // every cell — a clean repaint that wipes any residue.
            terminal.clear()?;
            self.needs_full_redraw = false;
        }
        let s = &self.state;
        let sidebar_active = s.focus_mode == FocusMode::Sidebar;
        let base_theme = *s.active_theme();
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
        // This one resolved modal drives both input routing and rendering. Even
        // if stale backing flags coexist, lower-priority overlays are never
        // painted over the modal that actually owns the keyboard and mouse.
        let active_modal = s.active_modal();
        let show_help = active_modal == Some(Modal::Help);
        let rename_input = if active_modal == Some(Modal::Rename) {
            s.overlay.renaming.as_ref().map(|r| &r.input)
        } else {
            None
        };
        let show_borders = s.prefs.show_borders;
        let sidebar_tab = s.prefs.sidebar_tab;
        let layout_mode = s.effective_layout_mode();
        let view_mode = s.prefs.view_mode;
        let sidebar_width = s.effective_sidebar_width();
        let sidebar_collapsed = s.prefs.sidebar_collapsed && layout_mode == LayoutMode::Horizontal;
        let sidebar_height = s.effective_sidebar_height();
        let main_view = s.main_view;
        let warning_state = self.warning_state.as_ref();
        let remote_placeholder = s.focused_remote_placeholder().map(|entry| {
            let origin = s.section_title(&entry.lane);
            let (title, detail) = match entry.kind {
                crate::state::SessionEntryKind::Connecting => (
                    format!("Connecting to @{origin}"),
                    "Waiting for the remote terminal to connect".to_string(),
                ),
                crate::state::SessionEntryKind::Unreachable => (
                    format!("Cannot reach @{origin}"),
                    "Reconnect this host from the sidebar".to_string(),
                ),
                crate::state::SessionEntryKind::NoSessions => (
                    format!("No sessions for @{origin}"),
                    "Create one from the host menu to attach here".to_string(),
                ),
                // A focused remote placeholder is never `Live`, but keep a
                // sensible fallback string rather than panic.
                crate::state::SessionEntryKind::Live { .. } => (
                    format!("No attachable session for @{origin}"),
                    "Create one from the host menu to attach here".to_string(),
                ),
            };
            (title, detail)
        });

        let confirm_name = (active_modal == Some(Modal::ConfirmKill))
            .then(|| s.confirm_kill_name())
            .flatten();

        let update_available = s.update_available.as_ref();
        let reload_status = s.reload_status.as_ref();
        let dragging_sep = s.dragging_separator;

        let mut captured_hits = crate::geometry::HitRegions::default();
        let mut captured_summary_popup_max_scroll: usize = 0;
        terminal.draw(|frame| {
            let full = frame.area();
            let reload_height = ui::reload_row_count(reload_status, full.width);
            // Paint the reload bar as an overlay, not its own layout slot:
            // keeping the content area full-height means PTY sizing (see
            // `AppState::pty_size`) and mouse routing stay stable when the bar
            // pops in.
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

            // ~12.5 fps braille spinner for the Summary card; sessions.rs
            // takes this mod the frame count.
            let spinner_idx = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| (d.as_millis() / 80) as usize)
                .unwrap_or(0);

            // The Summary card shows how long ago its text landed; compute the
            // "Xm ago" age here so the renderer stays free of wall-clock reads.
            let summary_age = match &self.state.summary.state {
                crate::summary_card::SummaryState::Ready { generated_at, .. } => {
                    Some(crate::update::relative_age(
                        crate::update::now_secs().saturating_sub(*generated_at),
                    ))
                }
                _ => None,
            };

            let layout = self.state.current_layout(view_mode);
            let agent_entries = self.state.agent_entries.as_slice();
            let focus_target = self.state.focus_target();
            let project_drag = self.state.project_drag_indicators();
            let summary_card_height = self.state.summary_card_height();
            captured_hits = if sidebar_collapsed {
                ui::draw_collapsed_sidebar(frame, sidebar_area, theme, show_borders)
            } else {
                ui::draw_sidebar(
                    frame,
                    sidebar_area,
                    ui::SidebarProps {
                        sessions: &self.state.entries,
                        built: &layout,
                        focus_target,
                        project_drag,
                        sidebar_active,
                        theme,
                        show_help,
                        confirm_kill: confirm_name.as_deref(),
                        rename_input,
                        show_borders,
                        sidebar_tab,
                        agent_entries,
                        summary: &self.state.summary.state,
                        summary_age: summary_age.as_deref(),
                        spinner_idx,
                        summary_scroll: self.state.summary.scroll,
                        summary_card_height,
                        tabs_mode: layout_mode == LayoutMode::Vertical,
                        keybindings: &self.state.keybindings,
                        update_available,
                    },
                )
            };

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
            let active_lane = self.attachments.active_lane().clone();
            let attachment_failure = self.attachments.failure(&active_lane).map(str::to_string);
            let active_attachment_dead = self.active_terminal().is_none();
            let screen = self.active_terminal().map(|surface| surface.screen());
            let upgrade_screen = match main_view {
                MainView::Upgrade => self
                    .upgrade_instance
                    .as_ref()
                    .map(|surface| surface.screen()),
                _ => None,
            };
            let background_screen = match (warning_state, main_view) {
                (Some(_), _) => None,
                (None, MainView::Terminal) if remote_placeholder.is_some() => None,
                // Dead local pane (no sessions to attach to) renders the
                // empty-state placeholder below instead of a stale screen.
                (None, MainView::Terminal) if active_attachment_dead => None,
                (None, MainView::Terminal) => screen,
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
                main_inner
            } else {
                frame.render_widget(Block::default().style(main_base), main_area);
                main_area
            };

            if let Some(screen) = background_screen {
                bridge::render_screen(screen, main_inner, frame.buffer_mut(), theme.text, theme.bg);
                if !sidebar_active && warning_state.is_none() {
                    bridge::set_cursor(frame, screen, main_inner);
                }
            }

            // deck stays open on a dead local pane instead of quitting.
            if warning_state.is_none() && main_view == MainView::Terminal && active_attachment_dead
            {
                let is_primary = active_lane == *self.attachments.primary_lane();
                let title = if is_primary {
                    "No local sessions"
                } else {
                    "Attachment unavailable"
                };
                let detail = attachment_failure.as_deref().unwrap_or(if is_primary {
                    "Create one from the sidebar to attach here"
                } else {
                    "Reconnect this lane from its sidebar divider"
                });
                draw_center_message(frame, main_inner, title, detail, theme);
            }

            if warning_state.is_none() && main_view == MainView::Terminal {
                if let Some((title, detail)) = remote_placeholder.as_ref() {
                    draw_center_message(frame, main_inner, title, detail, theme);
                }
            }

            // Built lazily — the row closures allocate help strings each frame.
            if warning_state.is_none() && main_view == MainView::Settings {
                ui::draw_settings_page(frame, main_inner, &self.build_settings_view(), theme);
            }

            if let Some(warning_state) = warning_state {
                let main_style = Style::default().fg(theme.text).add_modifier(Modifier::BOLD);
                let sub_style = Style::default().fg(theme.dim);

                let warning = Paragraph::new(vec![
                    Line::from(Span::styled(warning_state.text, main_style)),
                    Line::raw(""),
                    Line::from(Span::styled(warning_state.detail.as_str(), sub_style)),
                ])
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_set(ratatui::symbols::border::ROUNDED)
                        .title(" Heads up ")
                        .border_style(Style::default().fg(theme.yellow)),
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

            let rendered_modal = draw_active_modal(frame, s, full, main_inner, layout_mode, theme);
            captured_summary_popup_max_scroll = rendered_modal.summary_popup_max_scroll;
            captured_hits.new_session_dirs = rendered_modal.new_session_dirs;
            captured_hits.new_session_create = rendered_modal.new_session_create;
            if rendered_modal.kill_hits.is_some() {
                captured_hits.kill = rendered_modal.kill_hits;
            }

            // Overlay the reload bar last so it sits atop the sidebar footer,
            // main pane, warning popup, and context menu. Underlying layouts
            // keep their full area, so PTY sizing and mouse routing are
            // unaffected.
            if let (Some(status), Some(area)) = (reload_status, reload_area) {
                frame.render_widget(Clear, area);
                ui::draw_reload_bar(frame, area, status, theme);
            }
        })?;

        self.state.hit_regions = captured_hits;
        self.state.summary.popup_max_scroll = captured_summary_popup_max_scroll;

        Ok(())
    }

    /// The active page's descriptor rows reduced to display strings. Done here, holding
    /// `&AppState`, so `draw_settings_page` stays a pure `ui` fn over
    /// `Vec<SettingRowView>`.
    fn build_settings_view(&self) -> SettingsView {
        let s = &self.state;
        let page = s.settings.current_page();
        let source_rows = setting_rows(s);
        let rows: Vec<SettingRowView> = source_rows
            .into_iter()
            .map(|row| SettingRowView {
                label: row.label,
                value: (row.value)(s),
                help: (row.help)(s),
            })
            .collect();
        SettingsView {
            selected: s.settings.selected(),
            rows,
            page,
        }
    }
}

fn draw_center_message(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    detail: &str,
    theme: &crate::theme::Theme,
) {
    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(title, Style::default().fg(theme.text))),
        Line::from(Span::styled(detail, Style::default().fg(theme.dim))),
    ];
    let placeholder = Paragraph::new(lines)
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: true });
    frame.render_widget(placeholder, area);
}
