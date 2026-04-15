use std::io;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyEventKind};
use portable_pty::PtySize;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::DefaultTerminal;

use crate::action::{self, Action};
use crate::bridge;
use crate::config::Config;
use crate::git;
use crate::nesting_guard::{NestingGuard, WarningState};
use crate::pty::{Pty, PtyEvent};
use crate::state::{
    AppState, FocusMode, LayoutMode, MainView, SessionRow, ViewMode, SIDEBAR_MAX, SIDEBAR_MIN,
};
use crate::theme::THEMES;
use crate::tmux;
use crate::ui::{self, SessionView, SettingsView};

const POLL_MS: u64 = 16;
const REFRESH_INTERVAL: Duration = Duration::from_secs(1);

pub struct App {
    state: AppState,
    pty: Pty,
    parser: vt100::Parser,
    spinner: rattles::Rattler<rattles::presets::braille::Dots>,
    nesting_guard: NestingGuard,
    warning_state: Option<WarningState>,
}

impl App {
    fn centered_rect(area: Rect, width: u16, height: u16) -> Rect {
        let popup_width = width.min(area.width);
        let popup_height = height.min(area.height);
        let x = area.x + area.width.saturating_sub(popup_width) / 2;
        let y = area.y + area.height.saturating_sub(popup_height) / 2;
        Rect::new(x, y, popup_width, popup_height)
    }

    fn warning_blocks_action(action: &Action) -> bool {
        matches!(
            action,
            Action::SetFocusMain
                | Action::ToggleFocus
                | Action::ForwardKey(_)
                | Action::ForwardMouse(_)
        )
    }

    pub fn new(term_width: u16, term_height: u16) -> io::Result<Self> {
        let cfg = Config::load();

        let theme_index = THEMES.iter().position(|t| t.name == cfg.theme).unwrap_or(0);
        let layout_mode = match cfg.layout.as_str() {
            "vertical" => LayoutMode::Vertical,
            _ => LayoutMode::Horizontal,
        };
        let show_borders = cfg.show_borders;
        let view_mode = match cfg.view_mode.as_str() {
            "compact" => ViewMode::Compact,
            _ => ViewMode::Expanded,
        };
        let sidebar_width = cfg.sidebar_width.clamp(SIDEBAR_MIN, SIDEBAR_MAX);

        let exclude_patterns = cfg.exclude_patterns.clone();
        let compiled_patterns = crate::config::compile_patterns(&exclude_patterns);

        let state = AppState::new(
            theme_index,
            layout_mode,
            view_mode,
            show_borders,
            sidebar_width,
            term_width,
            term_height,
            exclude_patterns,
            compiled_patterns,
        );
        let nesting_guard = NestingGuard::new();

        let (pty_rows, pty_cols) = state.pty_size();
        let pty = Self::spawn_tmux_pty((pty_rows, pty_cols), &nesting_guard)?;
        let parser = vt100::Parser::new(pty_rows, pty_cols, 0);

        let mut app = App {
            state,
            pty,
            parser,
            spinner: rattles::presets::braille::dots(),
            nesting_guard,
            warning_state: None,
        };

        app.refresh_sessions();
        if let Some(pos) = app
            .state
            .filtered
            .iter()
            .position(|&i| app.state.sessions[i].is_current)
        {
            app.state.focused = pos;
        }

        Ok(app)
    }

    fn spawn_tmux_pty(size: (u16, u16), nesting_guard: &NestingGuard) -> io::Result<Pty> {
        let target = Self::ensure_attach_target(nesting_guard)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no tmux session to attach"))?;
        let args = ["attach", "-t", target.as_str()];
        Pty::spawn(
            "tmux",
            &args,
            PtySize {
                rows: size.0,
                cols: size.1,
                pixel_width: 0,
                pixel_height: 0,
            },
        )
    }

    fn ensure_attach_target(nesting_guard: &NestingGuard) -> Option<String> {
        let sessions = tmux::list_sessions();
        if let Some(name) = nesting_guard.preferred_attach_target(&sessions) {
            return Some(name);
        }

        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        let dir = format!("{}/claude", home);
        let mut idx = sessions.len();
        let name = loop {
            let candidate = format!("session-{}", idx);
            if !sessions.iter().any(|session| session.name == candidate) {
                break candidate;
            }
            idx += 1;
        };

        tmux::new_session(&name, &dir)?;
        Some(name)
    }

    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        let mut last_refresh = Instant::now();
        let mut pty_alive = true;

        loop {
            // 1. Drain PTY output
            for event in self.pty.drain() {
                match event {
                    PtyEvent::Output(data) => self.parser.process(&data),
                    PtyEvent::Exited => pty_alive = false,
                }
            }

            // 2. Render
            self.render(terminal)?;

            // 3. Poll input and dispatch
            if event::poll(Duration::from_millis(POLL_MS))? {
                match event::read()? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        let action = action::key_to_action(&key, &self.state);
                        if self.warning_state.is_some() && Self::warning_blocks_action(&action) {
                            self.state.focus_mode = FocusMode::Sidebar;
                            continue;
                        }
                        if self.dispatch(action) {
                            break;
                        }
                    }
                    Event::Mouse(mouse) => {
                        let action = action::mouse_to_action(&mouse, &self.state);
                        if self.warning_state.is_some() && Self::warning_blocks_action(&action) {
                            continue;
                        }
                        if self.dispatch(action) {
                            break;
                        }
                    }
                    Event::Resize(w, h) => {
                        self.dispatch(Action::Resize(w, h));
                    }
                    _ => {}
                }
            }

            // 4. Periodic refresh
            if last_refresh.elapsed() >= REFRESH_INTERVAL {
                self.refresh_sessions();
                last_refresh = Instant::now();
            }

            // 5. If PTY died, try to reattach
            if !pty_alive {
                if tmux::list_sessions().is_empty() {
                    break;
                }
                match self.respawn_pty() {
                    Ok(()) => {
                        pty_alive = true;
                        self.refresh_sessions();
                    }
                    Err(_) => break,
                }
            }
        }

        Ok(())
    }

    /// Dispatch an action through the pipeline. Returns true if the app should exit.
    fn dispatch(&mut self, action: Action) -> bool {
        match action {
            // PTY passthrough — no state change
            Action::ForwardKey(ref bytes) => {
                let _ = self.pty.write(bytes);
                return false;
            }
            Action::ForwardMouse(ref bytes) => {
                let _ = self.pty.write(bytes);
                self.state.focus_mode = FocusMode::Main;
                return false;
            }

            // Compound: sidebar click → focus sidebar + select + switch
            Action::SidebarClickSession(idx) => {
                action::apply_action(&mut self.state, Action::SetFocusSidebar);
                action::apply_action(&mut self.state, Action::FocusIndex(idx));
                let fx = action::apply_action(&mut self.state, Action::SwitchProject);
                self.execute_side_effects(&fx);
                return false;
            }

            // Compound: number key → focus + switch + go to main
            Action::NumberKeyJump(idx) => {
                action::apply_action(&mut self.state, Action::FocusIndex(idx));
                let fx = action::apply_action(&mut self.state, Action::SwitchProject);
                self.execute_side_effects(&fx);
                if self.warning_state.is_none() {
                    self.state.focus_mode = FocusMode::Main;
                }
                return false;
            }

            // Compound: Enter → switch + go to main
            Action::SwitchProject => {
                let fx = action::apply_action(&mut self.state, action);
                self.execute_side_effects(&fx);
                if self.warning_state.is_some() {
                    self.state.focus_mode = FocusMode::Sidebar;
                } else {
                    self.state.focus_mode = FocusMode::Main;
                }
                return fx.quit;
            }

            // Compound: context menu click → select item + confirm
            Action::MenuClickItem(idx) => {
                action::apply_action(&mut self.state, Action::MenuHover(idx));
                let fx = action::apply_action(&mut self.state, Action::MenuConfirm);
                self.execute_side_effects(&fx);
                if self.warning_state.is_some() {
                    self.state.focus_mode = FocusMode::Sidebar;
                }
                return fx.quit;
            }

            // All simple actions
            _ => {
                let fx = action::apply_action(&mut self.state, action);
                self.execute_side_effects(&fx);
                fx.quit
            }
        }
    }

    fn switch_client(&self, session: &str) {
        if self.pty.slave_tty.is_empty() {
            tmux::switch_session(session);
        } else {
            tmux::switch_client_for_tty(&self.pty.slave_tty, session);
        }
    }

    fn switch_to_session_if_safe(&mut self, session: &str) -> bool {
        if let Some(warning) = self.nesting_guard.warning_for_switch(session) {
            self.warning_state = Some(warning);
            return false;
        }

        self.warning_state = None;
        self.switch_client(session);
        true
    }

    fn execute_side_effects(&mut self, fx: &crate::state::SideEffect) {
        self.nesting_guard.refresh();

        if let Some(ref name) = fx.switch_session {
            self.switch_to_session_if_safe(name);
        }

        if let Some(ref rename) = fx.rename_session {
            tmux::rename_session(&rename.old_name, &rename.new_name);
            // Update session order to track the new name
            if let Some(pos) = self
                .state
                .session_order
                .iter()
                .position(|n| n == &rename.old_name)
            {
                self.state.session_order[pos] = rename.new_name.clone();
            }
        }

        if let Some(ref kill) = fx.kill_session {
            if let Some(ref alt_name) = kill.switch_to {
                self.switch_to_session_if_safe(alt_name);
            }
            tmux::kill_session(&kill.name);
        }

        if fx.create_session {
            self.create_new_session();
        }

        if fx.resize_pty {
            self.resize_pty();
        }

        if fx.save_config {
            self.save_config();
        }

        if fx.refresh_sessions {
            self.refresh_sessions();
        }
    }

    fn create_new_session(&mut self) {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        let dir = format!("{}/claude", home);
        let existing: Vec<&str> = self
            .state
            .sessions
            .iter()
            .map(|s| s.name.as_str())
            .collect();
        let mut idx = self.state.sessions.len();
        let name = loop {
            let candidate = format!("session-{}", idx);
            if !existing.contains(&candidate.as_str()) {
                break candidate;
            }
            idx += 1;
        };
        if tmux::new_session(&name, &dir).is_some() {
            self.switch_client(&name);
        }
    }

    fn render(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        let s = &self.state;
        let sidebar_active = s.focus_mode == FocusMode::Sidebar;
        let focused = s.focused;
        let theme = &THEMES[s.theme_index];
        let filter_mode = s.filter_mode;
        let confirm_kill = s.confirm_kill;
        let show_help = s.show_help;
        let rename_input = s.renaming.as_ref().map(|r| (r.input.clone(), r.cursor));
        let context_menu = s.context_menu.clone();
        let show_borders = s.show_borders;
        let layout_mode = s.layout_mode;
        let sidebar_width = s.sidebar_width;
        let main_view = s.main_view;
        let warning_state = self.warning_state.clone();

        let confirm_name = if confirm_kill {
            s.filtered
                .get(s.focused)
                .map(|&i| s.sessions[i].name.clone())
        } else {
            None
        };

        let views_owned: Vec<SessionRow> =
            s.filtered.iter().map(|&i| s.sessions[i].clone()).collect();

        let spinner_frame = self.spinner.current_frame().to_string();
        let settings_view = SettingsView {
            selected: s.settings_selected,
            focus_main: s.focus_mode == FocusMode::Main,
            theme_name: THEMES[s.theme_index].name,
            theme_picker_open: s.theme_picker_open,
            theme_picker_selected: s.theme_picker_selected,
            theme_names: THEMES.iter().map(|theme| theme.name).collect(),
            layout_mode: s.layout_mode,
            show_borders: s.show_borders,
            exclude_count: s.exclude_patterns.len(),
            exclude_editor: s.exclude_editor.as_ref().map(|e| ui::ExcludeEditorView {
                patterns: &s.exclude_patterns,
                selected: e.selected,
                adding: e.adding,
                input: &e.input,
                cursor: e.cursor,
                error: e.error.as_deref(),
            }),
        };
        let hover_sep = s.hover_separator;
        let dragging_sep = s.dragging_separator;

        terminal.draw(|frame| {
            let views: Vec<SessionView> = views_owned
                .iter()
                .map(|r| SessionView {
                    name: r.name.as_str(),
                    dir: r.dir.as_str(),
                    branch: r.branch.as_str(),
                    ahead: r.ahead,
                    behind: r.behind,
                    staged: r.staged,
                    modified: r.modified,
                    untracked: r.untracked,
                    is_current: r.is_current,
                    idle_seconds: r.idle_seconds,
                })
                .collect();

            let (sidebar_area, gap_area, main_area) = match layout_mode {
                LayoutMode::Horizontal => {
                    let [s, g, m] = Layout::horizontal([
                        Constraint::Length(sidebar_width),
                        Constraint::Length(1),
                        Constraint::Min(1),
                    ])
                    .areas(frame.area());
                    (s, Some(g), m)
                }
                LayoutMode::Vertical => {
                    let tab_h = if show_borders { 4u16 } else { 2u16 };
                    let [s, m] = Layout::vertical([Constraint::Length(tab_h), Constraint::Min(1)])
                        .areas(frame.area());
                    (s, None, m)
                }
            };

            ui::draw_sidebar(
                frame,
                sidebar_area,
                &views,
                focused,
                sidebar_active,
                theme,
                filter_mode,
                show_help,
                confirm_name.as_deref(),
                rename_input.as_ref().map(|(s, c)| (s.as_str(), *c)),
                show_borders,
                layout_mode == LayoutMode::Vertical,
                &spinner_frame,
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
                        cell.set_style(ratatui::style::Style::default().fg(sep_fg));
                    }
                }
            }

            let screen = self.parser.screen();
            let background_screen = match (warning_state.as_ref(), main_view) {
                (Some(WarningState::Proactive { .. } | WarningState::Detected(_)), _) => None,
                (None, MainView::Terminal) => Some(screen),
                (None, MainView::Settings) => None,
            };

            let main_inner = if show_borders {
                let main_border_color = if sidebar_active {
                    theme.dim
                } else {
                    theme.accent
                };
                let main_block = Block::default()
                    .borders(Borders::ALL)
                    .border_set(ratatui::symbols::border::ROUNDED)
                    .border_style(Style::default().fg(main_border_color));
                let main_inner = main_block.inner(main_area);
                frame.render_widget(main_block, main_area);
                if let Some(screen) = background_screen {
                    bridge::render_screen(screen, main_inner, frame.buffer_mut());
                    if !sidebar_active && warning_state.is_none() {
                        bridge::set_cursor(frame, screen, main_inner);
                    }
                }
                main_inner
            } else {
                if let Some(screen) = background_screen {
                    bridge::render_screen(screen, main_area, frame.buffer_mut());
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
                        WarningState::Proactive { text, detail } => (
                            " Heads up ",
                            theme.yellow,
                            Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
                            Style::default().fg(theme.dim),
                            text.to_string(),
                            detail,
                        ),
                        WarningState::Detected(text) => (
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
        })?;

        Ok(())
    }

    fn refresh_sessions(&mut self) {
        self.nesting_guard.refresh();
        let current = if self.pty.slave_tty.is_empty() {
            tmux::current_session()
        } else {
            tmux::current_session_for_tty(&self.pty.slave_tty)
        }
        .unwrap_or_default();

        if let Some(warning) = self
            .nesting_guard
            .warning_for_current_session(Some(current.as_str()))
        {
            self.warning_state = Some(warning);
        } else if matches!(self.warning_state, Some(WarningState::Detected(_))) {
            self.warning_state = None;
        }

        let sessions = tmux::list_sessions();

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        self.state.sessions = sessions
            .into_iter()
            .filter(|s| !crate::config::session_excluded(&s.name, &self.state.compiled_patterns))
            .map(|s| {
                let git_info = git::get_git_info(&s.dir);
                let idle_seconds = now.saturating_sub(s.activity);

                SessionRow {
                    is_current: s.name == current,
                    name: s.name,
                    dir: s.dir,
                    branch: git_info.branch,
                    ahead: git_info.ahead,
                    behind: git_info.behind,
                    staged: git_info.staged,
                    modified: git_info.modified,
                    untracked: git_info.untracked,
                    idle_seconds,
                }
            })
            .collect();

        self.state.sync_order();
        self.state.apply_order();
        self.state.recompute_filter();

        if self.state.focus_mode != FocusMode::Sidebar || self.state.current_session != current {
            if let Some(pos) = self
                .state
                .filtered
                .iter()
                .position(|&i| self.state.sessions[i].is_current)
            {
                self.state.focused = pos;
            }
        }

        self.state.current_session = current;

        if !self.state.filtered.is_empty() && self.state.focused >= self.state.filtered.len() {
            self.state.focused = self.state.filtered.len() - 1;
        }
    }

    fn resize_pty(&mut self) {
        let (pty_rows, pty_cols) = self.state.pty_size();
        self.parser.screen_mut().set_size(pty_rows, pty_cols);
        let _ = self.pty.resize(PtySize {
            rows: pty_rows,
            cols: pty_cols,
            pixel_width: 0,
            pixel_height: 0,
        });
    }

    fn respawn_pty(&mut self) -> io::Result<()> {
        let (pty_rows, pty_cols) = self.state.pty_size();
        self.nesting_guard.refresh();
        self.pty = Self::spawn_tmux_pty((pty_rows, pty_cols), &self.nesting_guard)?;
        self.parser = vt100::Parser::new(pty_rows, pty_cols, 0);
        Ok(())
    }

    fn save_config(&self) {
        Config {
            theme: THEMES[self.state.theme_index].name.to_string(),
            layout: match self.state.layout_mode {
                LayoutMode::Horizontal => "horizontal",
                LayoutMode::Vertical => "vertical",
            }
            .to_string(),
            show_borders: self.state.show_borders,
            sidebar_width: self.state.sidebar_width,
            view_mode: match self.state.view_mode {
                ViewMode::Expanded => "expanded",
                ViewMode::Compact => "compact",
            }
            .to_string(),
            exclude_patterns: self.state.exclude_patterns.clone(),
        }
        .save();
    }
}

#[cfg(test)]
mod tests {
    use super::App;
    use crate::action::Action;

    #[test]
    fn warning_only_blocks_main_pane_actions() {
        assert!(App::warning_blocks_action(&Action::SetFocusMain));
        assert!(App::warning_blocks_action(&Action::ToggleFocus));
        assert!(App::warning_blocks_action(&Action::ForwardMouse(vec![])));
        assert!(!App::warning_blocks_action(&Action::FocusNext));
        assert!(!App::warning_blocks_action(&Action::SwitchProject));
    }
}
