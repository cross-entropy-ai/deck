use std::io::{self, Write};
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyEventKind};
use portable_pty::PtySize;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::DefaultTerminal;

use std::collections::BTreeMap;

use crate::action::{self, Action};
use crate::bridge;
use crate::config::{Config, KeyBindingValue};
use crate::keybindings::Keybindings;
use crate::nesting_guard::{NestingGuard, WarningState};
use crate::pty::{Pty, PtyEvent};
use crate::refresh::{RefreshRequest, RefreshWorker, SessionSnapshot};
use crate::state::{
    AppState, FocusMode, LayoutMode, MainView, SessionRow, SideEffect, SIDEBAR_MAX, SIDEBAR_MIN,
};
use crate::theme::THEMES;
use crate::tmux;
use crate::ui::{self, SessionView, SettingsView};
use crate::update::{
    self, UpdateCache, UpdateCheckMode, UpdateChecker, UpdateRequest, UpdateResult, CACHE_TTL_SECS,
};

const POLL_MS: u64 = 16;
const REFRESH_INTERVAL: Duration = Duration::from_secs(1);
const UPDATE_CHECK_INTERVAL: Duration = Duration::from_secs(24 * 3600);

struct PluginInstance {
    pty: Pty,
    parser: vt100::Parser,
    alive: bool,
}

pub struct App {
    state: AppState,
    pty: Pty,
    parser: vt100::Parser,
    spinner: rattles::Rattler<rattles::presets::braille::Dots>,
    nesting_guard: NestingGuard,
    warning_state: Option<WarningState>,
    plugin_instances: Vec<Option<PluginInstance>>,
    refresh_worker: RefreshWorker,
    raw_keybindings: BTreeMap<String, KeyBindingValue>,
    update_checker: Option<UpdateChecker>,
    upgrade_instance: Option<PluginInstance>,
    last_update_request: Option<Instant>,
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
        let mut cfg = Config::load();

        // Backfill defaults for any config entries the user hasn't listed.
        // This makes ~/.config/deck/config.json self-documenting: after first
        // launch the file shows every option and its current value.
        let before = cfg.to_json();
        crate::keybindings::ensure_complete(&mut cfg.keybindings);
        if cfg.to_json() != before {
            cfg.save();
        }

        let theme_index = THEMES.iter().position(|t| t.name == cfg.theme).unwrap_or(0);
        let layout_mode = cfg.layout;
        let show_borders = cfg.show_borders;
        let view_mode = cfg.view_mode;
        let sidebar_width = cfg.sidebar_width.clamp(SIDEBAR_MIN, SIDEBAR_MAX);
        let sidebar_height = cfg.sidebar_height;

        let exclude_patterns = cfg.exclude_patterns.clone();
        let plugins = cfg.plugins.clone();
        let plugin_count = plugins.len();

        let (keybindings, kb_warnings) = Keybindings::from_config(&cfg.keybindings, &plugins);
        for warning in &kb_warnings {
            eprintln!("deck: {}", warning);
        }

        let mut state = AppState::new(
            theme_index,
            layout_mode,
            view_mode,
            show_borders,
            sidebar_width,
            sidebar_height,
            term_width,
            term_height,
            exclude_patterns,
            plugins,
            keybindings,
            cfg.update_check,
        );

        // Update check bootstrap: honor cache when fresh, otherwise spawn the
        // background checker. Only runs when the feature is enabled.
        let (update_checker, last_update_request) = if cfg.update_check == UpdateCheckMode::Enabled
        {
            bootstrap_update_check(&mut state)
        } else {
            (None, None)
        };

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
            plugin_instances: (0..plugin_count).map(|_| None).collect(),
            refresh_worker: RefreshWorker::spawn(),
            raw_keybindings: cfg.keybindings.clone(),
            update_checker,
            upgrade_instance: None,
            last_update_request,
        };

        tmux::apply_theme(&THEMES[theme_index]);
        app.request_refresh();

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
            // 1. Drain PTY output (tmux + plugins)
            for event in self.pty.drain() {
                match event {
                    PtyEvent::Output(data) => {
                        Self::forward_osc52(&data);
                        self.parser.process(&data);
                    }
                    PtyEvent::Exited => pty_alive = false,
                }
            }
            for inst in self.plugin_instances.iter_mut().flatten() {
                for event in inst.pty.drain() {
                    match event {
                        PtyEvent::Output(data) => inst.parser.process(&data),
                        PtyEvent::Exited => inst.alive = false,
                    }
                }
            }
            if let Some(ref mut inst) = self.upgrade_instance {
                for event in inst.pty.drain() {
                    match event {
                        PtyEvent::Output(data) => inst.parser.process(&data),
                        PtyEvent::Exited => inst.alive = false,
                    }
                }
            }

            // If the active plugin exited, return to terminal and clean up
            if let MainView::Plugin(idx) = self.state.main_view {
                if self
                    .plugin_instances
                    .get(idx)
                    .and_then(|o| o.as_ref())
                    .is_some_and(|inst| !inst.alive)
                {
                    self.plugin_instances[idx] = None;
                    self.state.main_view = MainView::Terminal;
                    self.state.focus_mode = FocusMode::Main;
                }
            }

            // Upgrade exit: clear the instance, return to terminal, clear the
            // banner. If brew actually upgraded, the new binary is only picked
            // up on the next deck launch — users will naturally see the
            // banner gone via the version check when they relaunch.
            if self.state.main_view == MainView::Upgrade
                && self
                    .upgrade_instance
                    .as_ref()
                    .is_some_and(|inst| !inst.alive)
            {
                self.upgrade_instance = None;
                self.state.main_view = MainView::Terminal;
                self.state.focus_mode = FocusMode::Main;
                self.state.update_available = None;
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
                    Event::Paste(text) => {
                        if self.state.focus_mode == FocusMode::Main {
                            let mut bytes = b"\x1b[200~".to_vec();
                            bytes.extend_from_slice(text.as_bytes());
                            bytes.extend_from_slice(b"\x1b[201~");
                            match self.state.main_view {
                                MainView::Terminal => {
                                    let _ = self.pty.write(&bytes);
                                }
                                MainView::Plugin(idx) => {
                                    if let Some(ref mut inst) =
                                        self.plugin_instances.get_mut(idx).and_then(|o| o.as_mut())
                                    {
                                        let _ = inst.pty.write(&bytes);
                                    }
                                }
                                MainView::Upgrade => {
                                    if let Some(ref mut inst) = self.upgrade_instance {
                                        let _ = inst.pty.write(&bytes);
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    Event::Resize(w, h) => {
                        self.dispatch(Action::Resize(w, h));
                    }
                    _ => {}
                }
            }

            // 4. Apply any snapshots produced by the refresh worker
            while let Some(snap) = self.refresh_worker.try_recv() {
                self.apply_snapshot(snap);
            }

            // 5. Periodic refresh request (non-blocking)
            if last_refresh.elapsed() >= REFRESH_INTERVAL {
                self.request_refresh();
                last_refresh = Instant::now();
            }

            // 6. Update check plumbing: handle mode transitions, recv results,
            //    schedule the 24h retry.
            self.tick_update_check();

            // 7. If PTY died, try to reattach
            if !pty_alive {
                if tmux::list_sessions().is_empty() {
                    break;
                }
                match self.respawn_pty() {
                    Ok(()) => {
                        pty_alive = true;
                        self.request_refresh();
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
            // PTY passthrough — route to tmux or active plugin
            Action::ForwardKey(ref bytes) => {
                match self.state.main_view {
                    MainView::Plugin(idx) => {
                        if let Some(Some(ref mut inst)) = self.plugin_instances.get_mut(idx) {
                            let _ = inst.pty.write(bytes);
                        }
                    }
                    MainView::Upgrade => {
                        if let Some(ref mut inst) = self.upgrade_instance {
                            let _ = inst.pty.write(bytes);
                        }
                    }
                    _ => {
                        let _ = self.pty.write(bytes);
                    }
                }
                return false;
            }
            Action::ForwardMouse(ref bytes) => {
                match self.state.main_view {
                    MainView::Plugin(idx) => {
                        if let Some(Some(ref mut inst)) = self.plugin_instances.get_mut(idx) {
                            let _ = inst.pty.write(bytes);
                        }
                    }
                    MainView::Upgrade => {
                        if let Some(ref mut inst) = self.upgrade_instance {
                            let _ = inst.pty.write(bytes);
                        }
                    }
                    _ => {
                        let _ = self.pty.write(bytes);
                    }
                }
                self.state.focus_mode = FocusMode::Main;
                return false;
            }

            // Compound: sidebar click → focus sidebar + select + switch
            Action::SidebarClickSession(idx) => {
                let mut fx = SideEffect::default();
                fx.merge(action::apply_action(
                    &mut self.state,
                    Action::SetFocusSidebar,
                ));
                fx.merge(action::apply_action(
                    &mut self.state,
                    Action::FocusIndex(idx),
                ));
                fx.merge(action::apply_action(&mut self.state, Action::SwitchProject));
                self.execute_side_effects(&fx);
                return false;
            }

            // Compound: number key → focus + switch + go to main
            Action::NumberKeyJump(idx) => {
                let mut fx = SideEffect::default();
                fx.merge(action::apply_action(
                    &mut self.state,
                    Action::FocusIndex(idx),
                ));
                fx.merge(action::apply_action(&mut self.state, Action::SwitchProject));
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
                let mut fx = SideEffect::default();
                fx.merge(action::apply_action(
                    &mut self.state,
                    Action::MenuHover(idx),
                ));
                fx.merge(action::apply_action(&mut self.state, Action::MenuConfirm));
                self.execute_side_effects(&fx);
                if self.warning_state.is_some() {
                    self.state.focus_mode = FocusMode::Sidebar;
                }
                return fx.quit;
            }

            // Plugin activation: lazy-spawn PTY then switch view
            Action::ActivatePlugin(idx) => {
                // Respawn if dead
                if let Some(Some(ref inst)) = self.plugin_instances.get(idx) {
                    if !inst.alive {
                        self.plugin_instances[idx] = None;
                    }
                }
                // Lazy spawn — only switch view if spawn succeeds
                if idx < self.plugin_instances.len() && self.plugin_instances[idx].is_none() {
                    if self.spawn_plugin_pty(idx).is_err() {
                        return false;
                    }
                }
                let fx = action::apply_action(&mut self.state, action);
                self.execute_side_effects(&fx);
                return fx.quit;
            }

            Action::TriggerUpgrade => {
                if self.state.update_available.is_none() {
                    return false;
                }
                if !update::has_brew() {
                    self.warning_state = Some(WarningState::Proactive {
                        text: "Homebrew not found",
                        detail: "Install from https://brew.sh, then retry.\n\
                                 Alternatively: cargo install --git https://github.com/cross-entropy-ai/deck"
                            .to_string(),
                    });
                    return false;
                }
                if let Err(e) = self.spawn_upgrade_pty() {
                    eprintln!("deck: failed to spawn upgrade: {}", e);
                    return false;
                }
                self.state.main_view = MainView::Upgrade;
                self.state.focus_mode = FocusMode::Main;
                return false;
            }

            Action::AbortUpgrade => {
                // Dropping the upgrade instance runs Pty::drop which kills
                // the child. No explicit signal needed.
                self.upgrade_instance = None;
                self.state.main_view = MainView::Terminal;
                self.state.focus_mode = FocusMode::Main;
                return false;
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

        if fx.apply_tmux_theme {
            tmux::apply_theme(&THEMES[self.state.theme_index]);
        }

        if fx.refresh_sessions {
            self.request_refresh();
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

        let views_owned: Vec<SessionRow> =
            s.filtered.iter().map(|&i| s.sessions[i].clone()).collect();

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
        let hover_sep = s.hover_separator;
        let dragging_sep = s.dragging_separator;

        let mut captured_banner_bounds: Option<Rect> = None;
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
                    let [s, m] =
                        Layout::vertical([Constraint::Length(sidebar_height), Constraint::Min(1)])
                            .areas(frame.area());
                    (s, None, m)
                }
            };

            let plugin_hints: Vec<(char, &str)> = self
                .state
                .plugins
                .iter()
                .map(|p| (p.key, p.name.as_str()))
                .collect();

            captured_banner_bounds = ui::draw_sidebar(
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
                view_mode,
                &plugin_hints,
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
                MainView::Upgrade => self.upgrade_instance.as_ref().map(|inst| inst.parser.screen()),
                _ => None,
            };
            let background_screen = match (warning_state.as_ref(), main_view) {
                (Some(WarningState::Proactive { .. } | WarningState::Detected(_)), _) => None,
                (None, MainView::Terminal) => Some(screen),
                (None, MainView::Plugin(_)) => plugin_screen,
                (None, MainView::Upgrade) => upgrade_screen,
                (None, MainView::Settings) => None,
            };

            // Base style for the main pane — Color::Default cells inherit this,
            // so the terminal background/foreground follows the deck theme.
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

        // Stash the banner-click hit-test region for the next mouse event.
        self.state.banner_upgrade_bounds = captured_banner_bounds;

        Ok(())
    }

    fn build_refresh_request(&self) -> RefreshRequest {
        RefreshRequest {
            slave_tty: self.pty.slave_tty.clone(),
            exclude_patterns: self.state.exclude_patterns.clone(),
        }
    }

    fn request_refresh(&mut self) {
        self.nesting_guard.refresh();
        self.refresh_worker.request(self.build_refresh_request());
    }

    fn apply_snapshot(&mut self, snap: SessionSnapshot) {
        let current = snap.current_session;

        if let Some(warning) = self
            .nesting_guard
            .warning_for_current_session(Some(current.as_str()))
        {
            self.warning_state = Some(warning);
        } else if matches!(self.warning_state, Some(WarningState::Detected(_))) {
            self.warning_state = None;
        }

        self.state.sessions = snap
            .rows
            .into_iter()
            .map(|r| SessionRow {
                is_current: r.name == current,
                name: r.name,
                dir: r.dir,
                branch: r.branch,
                ahead: r.ahead,
                behind: r.behind,
                staged: r.staged,
                modified: r.modified,
                untracked: r.untracked,
                idle_seconds: r.idle_seconds,
            })
            .collect();

        self.state.sync_order();
        self.state.apply_order();
        self.state.recompute_filter();

        // Snap focus to the current session only when current actually
        // changed — at boot current_session is empty so the first
        // snapshot lands correctly, and afterwards we preserve the
        // user's selection regardless of where their focus is.
        if self.state.current_session != current {
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
        // Resize all active plugin PTYs
        for inst in self.plugin_instances.iter_mut().flatten() {
            inst.parser.screen_mut().set_size(pty_rows, pty_cols);
            let _ = inst.pty.resize(PtySize {
                rows: pty_rows,
                cols: pty_cols,
                pixel_width: 0,
                pixel_height: 0,
            });
        }
    }

    /// Forward OSC 52 (clipboard) sequences directly to the real terminal.
    /// The vt100 parser discards these as unhandled, so programs running in
    /// the PTY (via tmux) can't set the clipboard without this passthrough.
    fn forward_osc52(data: &[u8]) {
        // OSC 52 starts with ESC ] 52 ; and ends with BEL (0x07) or ST (ESC \)
        let marker = b"\x1b]52;";
        let mut i = 0;
        while i + marker.len() <= data.len() {
            if data[i..].starts_with(marker) {
                let start = i;
                i += marker.len();
                // Scan for terminator
                while i < data.len() {
                    if data[i] == 0x07 {
                        // BEL terminated
                        let _ = io::stdout().write_all(&data[start..=i]);
                        let _ = io::stdout().flush();
                        break;
                    }
                    if data[i] == 0x1b && i + 1 < data.len() && data[i + 1] == b'\\' {
                        // ST terminated
                        let _ = io::stdout().write_all(&data[start..=i + 1]);
                        let _ = io::stdout().flush();
                        i += 1;
                        break;
                    }
                    i += 1;
                }
            }
            i += 1;
        }
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
            layout: self.state.layout_mode,
            show_borders: self.state.show_borders,
            sidebar_width: self.state.sidebar_width,
            sidebar_height: self.state.sidebar_height,
            view_mode: self.state.view_mode,
            exclude_patterns: self.state.exclude_patterns.clone(),
            plugins: self.state.plugins.clone(),
            keybindings: self.raw_keybindings.clone(),
            update_check: self.state.update_check_mode,
        }
        .save();
    }

    fn tick_update_check(&mut self) {
        // React to Settings toggling the mode at runtime.
        match self.state.update_check_mode {
            UpdateCheckMode::Disabled => {
                if self.update_checker.is_some() {
                    // Dropping the checker sends Shutdown and joins the thread.
                    self.update_checker = None;
                    self.last_update_request = None;
                }
                return;
            }
            UpdateCheckMode::Enabled => {
                if self.update_checker.is_none() {
                    // Was disabled; reboot the checker (no cache peek — the
                    // user just asked for a fresh check).
                    let checker = UpdateChecker::spawn();
                    checker.request(UpdateRequest::Check);
                    self.update_checker = Some(checker);
                    self.last_update_request = Some(Instant::now());
                }
            }
        }

        // Drain results.
        if let Some(ref checker) = self.update_checker {
            while let Some(result) = checker.try_recv() {
                match result {
                    UpdateResult::Ok {
                        status,
                        newer_than_current,
                    } => {
                        UpdateCache::save(&status);
                        self.state.update_last_checked_secs = Some(status.checked_at);
                        self.state.update_available =
                            if newer_than_current { Some(status) } else { None };
                    }
                    UpdateResult::Err(msg) => {
                        eprintln!("deck: update check failed: {}", msg);
                    }
                }
            }
        }

        // 24h retry.
        if let Some(last) = self.last_update_request {
            if last.elapsed() >= UPDATE_CHECK_INTERVAL {
                if let Some(ref checker) = self.update_checker {
                    checker.request(UpdateRequest::Check);
                    self.last_update_request = Some(Instant::now());
                }
            }
        }
    }

    fn spawn_upgrade_pty(&mut self) -> io::Result<()> {
        let (rows, cols) = self.state.pty_size();
        let pty = Pty::spawn_with_env(
            "brew",
            &["upgrade", "cross-entropy-ai/tap/deck"],
            PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            },
            &[("COLUMNS", &cols.to_string()), ("LINES", &rows.to_string())],
        )?;
        let parser = vt100::Parser::new(rows, cols, 0);
        self.upgrade_instance = Some(PluginInstance {
            pty,
            parser,
            alive: true,
        });
        Ok(())
    }

    fn spawn_plugin_pty(&mut self, idx: usize) -> io::Result<()> {
        let plugin = &self.state.plugins[idx];
        let (rows, cols) = self.state.pty_size();

        let parts: Vec<&str> = plugin.command.split_whitespace().collect();
        let (program, args) = parts
            .split_first()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "empty plugin command"))?;

        let pty = Pty::spawn_with_env(
            program,
            args,
            PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            },
            &[("COLUMNS", &cols.to_string()), ("LINES", &rows.to_string())],
        )?;
        let parser = vt100::Parser::new(rows, cols, 0);

        self.plugin_instances[idx] = Some(PluginInstance {
            pty,
            parser,
            alive: true,
        });
        Ok(())
    }
}

/// Render the help text shown under the Update check settings row.
/// Appends `· last checked Nh ago` when a cache timestamp is known.
fn format_update_check_help(last_checked_secs: Option<u64>) -> String {
    let base = "Left/right toggles auto update check";
    let Some(ts) = last_checked_secs else {
        return base.to_string();
    };
    let now = update::now_secs();
    let elapsed = now.saturating_sub(ts);
    let suffix = if elapsed < 60 {
        "just now".to_string()
    } else if elapsed < 3600 {
        format!("{}m ago", elapsed / 60)
    } else if elapsed < 86_400 {
        format!("{}h ago", elapsed / 3600)
    } else {
        format!("{}d ago", elapsed / 86_400)
    };
    format!("{} · last checked {}", base, suffix)
}

/// Set up update checking at startup. When the local cache is fresh
/// (<24h old), reuse its result and skip the network call. Otherwise
/// spawn the background checker and send an initial `Check`.
fn bootstrap_update_check(state: &mut AppState) -> (Option<UpdateChecker>, Option<Instant>) {
    let cached = UpdateCache::load();
    let now = update::now_secs();
    if let Some(ref status) = cached {
        state.update_last_checked_secs = Some(status.checked_at);
        if UpdateCache::is_fresh(status, now, CACHE_TTL_SECS) {
            // Cache is fresh — use it without hitting the network.
            match update::compare(&status.current_version, &status.latest_version) {
                Some(true) => {
                    // Update known to be available. Refresh current_version
                    // in case we've since upgraded and cache is stale by a
                    // point release.
                    if status.current_version == env!("CARGO_PKG_VERSION") {
                        state.update_available = Some(status.clone());
                    } else {
                        // Binary version shifted since cache was written.
                        // Drop cache and recheck.
                        return spawn_and_request_check();
                    }
                }
                _ => {
                    state.update_available = None;
                }
            }
            // Schedule the next 24h retry relative to when the cache was
            // written — not relative to now — so polling stays on cadence.
            let elapsed = now.saturating_sub(status.checked_at);
            let last_request = Instant::now()
                .checked_sub(Duration::from_secs(elapsed))
                .unwrap_or_else(Instant::now);
            return (None, Some(last_request));
        }
    }
    spawn_and_request_check()
}

fn spawn_and_request_check() -> (Option<UpdateChecker>, Option<Instant>) {
    let checker = UpdateChecker::spawn();
    checker.request(UpdateRequest::Check);
    (Some(checker), Some(Instant::now()))
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
