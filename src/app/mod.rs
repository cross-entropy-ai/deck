pub mod action;

mod dispatch;
mod lifecycle;
mod pty;
mod refresh;
mod render;
mod update;

use std::collections::BTreeMap;
use std::io;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyEventKind};
use ratatui::DefaultTerminal;

use crate::action::Action;
use crate::config::{Config, KeyBindingValue};
use crate::keybindings::Keybindings;
use crate::nesting_guard::{NestingGuard, WarningState};
use crate::pty::{Pty, PtyEvent};
use crate::refresh::RefreshWorker;
use crate::state::{AppState, FocusMode, MainView, SIDEBAR_MAX, SIDEBAR_MIN};
use crate::theme::THEMES;
use crate::tmux;
use crate::update::UpdateCheckMode;

use self::update::bootstrap_update_check;

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
    update_checker: Option<crate::update::UpdateChecker>,
    upgrade_instance: Option<PluginInstance>,
    last_update_request: Option<Instant>,
}

impl App {
    pub fn new(term_width: u16, term_height: u16) -> io::Result<Self> {
        let mut cfg = Config::load();

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

    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        let mut last_refresh = Instant::now();
        let mut pty_alive = true;

        loop {
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

            self.render(terminal)?;

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

            while let Some(snap) = self.refresh_worker.try_recv() {
                self.apply_snapshot(snap);
            }

            if last_refresh.elapsed() >= REFRESH_INTERVAL {
                self.request_refresh();
                last_refresh = Instant::now();
            }

            self.tick_update_check();

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
}
