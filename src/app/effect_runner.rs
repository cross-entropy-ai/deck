//! Imperative boundary for reducer-produced side effects.
//!
//! The dispatcher chooses actions and the reducer describes effects; this
//! module is the single place that translates those descriptions into App
//! services and runtime mutations.

use crate::action::{Action, MenuAction, PfAction};
use crate::effects::{Effect, SideEffect};
use crate::state::MainView;

use super::App;

impl App {
    pub(super) fn execute_side_effects(&mut self, effects: &SideEffect) {
        for effect in effects.effects() {
            match effect {
                Effect::ActivateSession(id) => self.activate_session(id.clone()),
                Effect::SwitchAgentPane(target) => self.switch_to_agent_pane(target.clone()),
                Effect::ShowRemotePlaceholder(host) => {
                    if self
                        .state
                        .focused_remote_placeholder()
                        .is_none_or(|entry| entry.host.as_deref() != Some(host.as_str()))
                    {
                        continue;
                    }
                    self.remote.clear_active();
                    self.state.main_view = MainView::Terminal;
                    self.supersede_agent_focus();
                    self.suppress_next_periodic_refresh = true;
                }
                Effect::RenameSession(rename) => {
                    let order_index = if self.state.is_primary_lane(&rename.lane) {
                        self.state
                            .session_order
                            .iter()
                            .position(|name| name == &rename.old_name)
                    } else {
                        None
                    };
                    self.submit_session(
                        rename.lane.clone(),
                        crate::session::executor::SessionOp::Rename {
                            old: rename.old_name.clone(),
                            new: rename.new_name.clone(),
                            order_index,
                        },
                    );
                }
                Effect::KillSession(kill) => {
                    if self.state.is_primary_lane(&kill.lane) {
                        if let Some(alternative) = &kill.switch_to {
                            self.switch_to_session_if_safe(kill.lane.clone(), alternative);
                        }
                    } else if let Some(host) = self.state.host_for_lane(&kill.lane) {
                        if self.remote.active_is(host) {
                            self.remote.clear_active();
                            self.needs_full_redraw = true;
                        }
                    }
                    self.submit_session(
                        kill.lane.clone(),
                        crate::session::executor::SessionOp::Kill {
                            name: kill.name.clone(),
                        },
                    );
                }
                Effect::CreateSession(req) => self.submit_session(
                    req.lane.clone(),
                    crate::session::executor::SessionOp::NewSession {
                        name: req.name.clone(),
                        dir: req.dir.clone(),
                    },
                ),
                Effect::ResizePty { full_redraw } => {
                    self.resize_pty();
                    self.needs_full_redraw |= *full_redraw;
                }
                Effect::SaveConfig => self.save_config(),
                Effect::SaveSessionOrder => {
                    let lane = self
                        .state
                        .local_entries()
                        .next()
                        .map(|entry| entry.lane.clone());
                    if let Some(lane) = lane {
                        self.submit_session(
                            lane,
                            crate::session::executor::SessionOp::PersistOrder {
                                order: self.state.session_order.clone(),
                            },
                        );
                    }
                }
                Effect::SaveRemoteSessionOrder(host) => {
                    let order = crate::state::attachable_on_host(&self.state.entries, Some(host))
                        .map(|entry| entry.name.clone())
                        .collect();
                    if let Some(lane) = self
                        .state
                        .entries
                        .iter()
                        .find(|entry| entry.host.as_deref() == Some(host.as_str()))
                        .map(|entry| entry.lane.clone())
                    {
                        self.submit_session(
                            lane,
                            crate::session::executor::SessionOp::PersistOrder { order },
                        );
                    }
                }
                Effect::RemoveRemoteHost(req) => {
                    let _ = self.port_forward_tx.send(
                        crate::app::ssh::port_forward_task::Op::StopHost {
                            host: req.host.clone(),
                        },
                    );
                    self.offboard_remote_host(&req.host, Some(&req.lane));
                }
                Effect::ReconnectHost(host) => {
                    self.dispatch(Action::ReconnectHost { host: host.clone() });
                }
                Effect::OpenForwardOverlay(host) => {
                    self.dispatch(Action::Pf(PfAction::Open(host.clone())));
                }
                Effect::OpenDividerMenu { host, x, y } => {
                    let action = match host {
                        Some(host) => Action::Menu(MenuAction::OpenHostDivider {
                            host: host.clone(),
                            x: *x,
                            y: *y,
                        }),
                        None => Action::Menu(MenuAction::OpenLocalDivider { x: *x, y: *y }),
                    };
                    self.dispatch(action);
                }
                Effect::ApplyTmuxTheme => crate::tmux::apply_theme(self.state.active_theme()),
                Effect::ProbeTerminalBg => self.probe_terminal_bg(),
                Effect::RefreshSessions => self.request_refresh(),
                Effect::RereadNewSessionEntries => self.request_new_session_listing(),
                Effect::OpenNewSessionPicker => self.open_new_session_picker(),
                Effect::OpenRemoteNewSessionPicker(host) => {
                    self.open_remote_new_session_picker(host);
                }
                Effect::OpenAddRemotePicker => self.open_add_remote_picker(),
                Effect::AddRemoteHost(host) => self.onboard_remote_host(host),
                Effect::Quit => {}
            }
        }
    }
}
