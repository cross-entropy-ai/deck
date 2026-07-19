use crate::new_session::textarea_line;
use crate::state::{
    AppState, FocusMode, KillRequest, LayoutMode, MainView, RemoteSwitchRequest, RenameRequest,
    RenameState, SideEffect, SidebarTab, ViewMode,
};

use super::{
    Action, AddRemoteAction, MenuAction, NewSessionAction, PfAction, SettingsAction, SummaryAction,
};

mod menu;
mod port_forward;
mod settings;

/// Tear down the settings page and its sub-overlays, returning the main pane
/// to the terminal. Called when focus moves to the sidebar while settings is
/// open: leaving the page for the sidebar closes it outright (same end state
/// as `Esc`). No-op when settings isn't showing.
fn close_settings_page(state: &mut AppState) {
    if state.main_view != MainView::Settings {
        return;
    }
    state.main_view = MainView::Terminal;
    state.settings.theme_picker_open = false;
    state.settings.keybindings_view_open = false;
    state.overlay.exclude_editor = None;
    state.overlay.summary_lang_input = None;
}

/// Fill the `SideEffect` field for the focused row — `switch_session` for a
/// local row, `switch_remote` for a remote one. Local-vs-remote dispatch reads
/// `entry.host` off the focused `SessionEntry` (via `AppState::entry_at`)
/// rather than taking apart the flat focus index.
fn fill_switch_effect(state: &AppState, fx: &mut SideEffect) -> bool {
    let Some(target) = state.focus_target() else {
        return false;
    };
    match state.entry_at(target) {
        Some(entry) if entry.is_local() => {
            fx.switch_session(entry.name.clone());
            true
        }
        // Reached only for non-local entries (the `is_local` arm caught
        // locals), which always have `host = Some`. Synthetic placeholder rows
        // (connecting, unreachable, "no sessions") have no real session — skip
        // silently so a click doesn't fire a doomed remote switch.
        Some(entry) if entry.is_attachable() => {
            fx.switch_remote(RemoteSwitchRequest {
                host: entry.host.clone().expect("non-local entry has a host"),
                name: entry.name.clone(),
            });
            true
        }
        Some(entry) => {
            fx.show_remote_placeholder(entry.host.clone().expect("non-local entry has a host"));
            false
        }
        None => false,
    }
}

/// Advance focus to the next visible row, skipping rows hidden in a collapsed
/// group. Clamps at the last visible row. Fills the switch effect when the
/// selection actually moved.
fn focus_next(state: &mut AppState, fx: &mut SideEffect) {
    let total = state.focusable_count();
    if total == 0 {
        return;
    }
    let old = state.cursor();
    let mut next = state.cursor();
    while next + 1 < total {
        next += 1;
        if !state.is_focus_collapsed(next) {
            state.set_cursor(next);
            if state.cursor() != old {
                switch_on_navigate(state, fx);
            }
            return;
        }
    }
    // No visible row below — stay put.
}

/// Move focus to the previous visible row, skipping rows hidden inside a
/// collapsed group. Fills the switch effect when the selection moved.
fn focus_prev(state: &mut AppState, fx: &mut SideEffect) {
    let mut prev = state.cursor();
    while prev > 0 {
        prev -= 1;
        if !state.is_focus_collapsed(prev) {
            state.set_cursor(prev);
            switch_on_navigate(state, fx);
            return;
        }
    }
    // No visible row above — stay put.
}

/// What a cursor move should switch to. The right pane follows the cursor so
/// the highlighted row matches what's shown: sessions on the Projects tab, the
/// agent's pane on the Agents tab (mirroring Projects, which switches even over
/// ssh for remote rows).
fn switch_on_navigate(state: &AppState, fx: &mut SideEffect) {
    if state.agents_tab_active() {
        fill_switch_agent_effect(state, fx);
    } else {
        fill_switch_effect(state, fx);
    }
}

/// Queue a switch to the agent under the Agents-tab cursor, if any.
/// Routed through `Effect::SwitchAgentPane` so dispatch focuses the pane
/// exactly like an agent-row click. Returns whether an agent was queued.
fn fill_switch_agent_effect(state: &AppState, fx: &mut SideEffect) -> bool {
    match state.focused_agent() {
        Some(target) => {
            fx.switch_agent_pane(target);
            true
        }
        None => false,
    }
}

/// Activate a sidebar tab. No-op if already active. Persists the choice.
///
/// Arriving on the Agents tab also (a) kicks a refresh so detection (gated on
/// the tab being active) starts at once, and (b) syncs the right pane to the
/// focused agent. If an agent is already active, the cursor lands on it first
/// so returning to the tab restores the position.
fn switch_tab(state: &mut AppState, fx: &mut SideEffect, tab: SidebarTab) {
    if state.prefs.sidebar_tab == tab {
        return;
    }
    state.prefs.sidebar_tab = tab;
    if tab == SidebarTab::Agents {
        if let Some(active) = state.active_agent.clone() {
            if let Some(idx) = state.agent_entry_index_for(&active) {
                state.agent_focused = idx;
            }
        }
        state.clamp_agent_focus();
        // Reflect the focused agent on the right so the highlight in the
        // panel and the active pane agree the instant the tab opens.
        fill_switch_agent_effect(state, fx);
        fx.refresh_sessions();
    }
    fx.save_config();
}

pub fn apply_action(state: &mut AppState, action: Action) -> SideEffect {
    let mut fx = SideEffect::default();

    match action {
        Action::FocusNext => focus_next(state, &mut fx),
        Action::FocusPrev => focus_prev(state, &mut fx),
        Action::ScrollUp => {
            state.last_scroll = std::time::Instant::now();
            focus_prev(state, &mut fx);
        }
        Action::ScrollDown => {
            state.last_scroll = std::time::Instant::now();
            focus_next(state, &mut fx);
        }
        Action::FocusIndex(idx) => {
            // Mouse clicks pass a unified flat index (local rows then remotes);
            // number-key shortcuts use the same action but stay inside the
            // local range. Either way `focusable_count` is the right bound.
            if idx < state.focusable_count() {
                state.set_cursor(idx);
            }
        }

        Action::SwitchProject => {
            if state.agents_tab_active() {
                // Agents tab: Enter (and number-jump) focuses the pane.
                fill_switch_agent_effect(state, &mut fx);
            } else if fill_switch_effect(state, &mut fx) {
                fx.refresh_sessions();
            }
        }
        Action::KillSession => {
            // Sessions only — the Agents tab has no kill action.
            if state.agents_tab_active() {
                return fx;
            }
            // Same policy the context menu uses to grey "Kill": no killing a
            // placeholder row, a host's last live session, or the last local
            // session. See `can_kill_focused`.
            if state.can_kill_focused() {
                state.overlay.confirm_kill = true;
            }
        }
        Action::ConfirmKill => {
            // Always dismiss the overlay first, even when the kill is then
            // blocked (defense in depth: a stale or forced confirm shouldn't
            // fire on a placeholder, a host's last session, or the last local
            // session — `can_kill_focused` gates that, same as KillSession).
            state.overlay.confirm_kill = false;
            if !state.can_kill_focused() {
                return fx;
            }
            let Some(target) = state.focus_target() else {
                return fx;
            };
            let Some(entry) = state.entry_at(target) else {
                return fx;
            };
            match entry.host.clone() {
                None => {
                    let name = entry.name.clone();
                    let killing_current = entry.is_current();
                    // Locals occupy the front of `entries`; the cursor is on
                    // a local row here (host == None), so its neighbors are
                    // also local. Clamp the neighbor search to the local block.
                    let local_count = state.local_count();

                    let next_focused = if state.focused + 1 < local_count {
                        state.focused
                    } else {
                        state.focused.saturating_sub(1)
                    };

                    // Pre-switch off the doomed session only when deck is
                    // attached to it. Killing a non-current row leaves the main
                    // view where it is — see KillRequest.switch_to.
                    let switch_to = if killing_current {
                        let alt_idx = if state.focused + 1 < local_count {
                            Some(state.focused + 1)
                        } else if state.focused > 0 {
                            Some(state.focused - 1)
                        } else {
                            None
                        };
                        alt_idx
                            .and_then(|i| state.entries.get(i))
                            .map(|e| e.name.clone())
                    } else {
                        None
                    };

                    state.session_order.retain(|n| n != &name);
                    state.focused = next_focused.min(local_count.saturating_sub(1));

                    fx.kill_session(KillRequest {
                        name,
                        host: None,
                        switch_to,
                    });
                    fx.refresh_sessions();
                }
                Some(host) => {
                    let name = entry.name.clone();
                    fx.kill_session(KillRequest {
                        name,
                        host: Some(host),
                        // No local switch_to: dispatch returns the
                        // user to local view after a remote kill.
                        switch_to: None,
                    });
                    fx.refresh_sessions();
                }
            }
        }
        Action::CancelKill => {
            state.overlay.confirm_kill = false;
        }
        Action::RemoveRemoteFromList(host) => {
            // Mirror `deck remote remove <host>` on the in-memory copy: drop
            // the host from config_remotes (save_config persists it) and clear
            // its session rows so the sidebar updates before the next refresh.
            // The host's forward *rules* ride inside its `RemoteConfig`, so
            // they're dropped here too.
            state.config_remotes.retain(|r| r.host != host);
            state
                .entries
                .retain(|e| e.host.as_deref() != Some(host.as_str()));
            state.clamp_projects_focus();
            state.clamp_agent_focus();
            fx.save_config();
            fx.refresh_sessions();
            fx.remove_remote_host(host);
        }
        Action::ReorderSession(direction) => {
            if state.agents_tab_active() {
                return fx;
            }
            // Remote row: reorder only within the same host's contiguous block
            // (hosts can't interleave). Swap with the adjacent row in
            // `direction` if it's the same host and a real session, then persist
            // that host's order over ssh. `focused` indexes `entries` directly.
            let idx = state.focused;
            let len = state.entries.len();
            let Some(entry) = state.entries.get(idx) else {
                return fx;
            };
            if let Some(host) = entry.host.clone() {
                if !entry.is_attachable() {
                    return fx;
                }
                let neighbor = idx as i32 + direction;
                if neighbor < 0 || neighbor as usize >= len {
                    return fx;
                }
                let neighbor = neighbor as usize;
                let n = &state.entries[neighbor];
                if n.host.as_deref() != Some(host.as_str()) || !n.is_attachable() {
                    return fx;
                }
                state.entries.swap(idx, neighbor);
                state.focused = neighbor;
                fx.save_remote_session_order(host);
                return fx;
            }

            let name = entry.name.clone();
            if let Some(pos) = state.session_order.iter().position(|n| n == &name) {
                let new_pos = (pos as i32 + direction)
                    .clamp(0, state.session_order.len() as i32 - 1)
                    as usize;
                if new_pos != pos {
                    state.session_order.swap(pos, new_pos);
                    state.apply_order();
                    state.clamp_projects_focus();
                    if let Some(new_focused) = state
                        .entries
                        .iter()
                        .position(|e| e.is_local() && e.name == name)
                    {
                        state.focused = new_focused;
                    }
                    // Persist the new arrangement onto the tmux sessions so
                    // it survives a deck restart (see `persist_session_order`).
                    fx.save_session_order();
                }
            }
        }
        Action::StartRename => {
            if state.agents_tab_active() {
                return fx;
            }
            let Some(target) = state.focus_target() else {
                return fx;
            };
            let Some(entry) = state.entry_at(target) else {
                return fx;
            };
            // Don't rename a synthetic placeholder row (no real session).
            if !entry.is_attachable() {
                return fx;
            }
            let name = entry.name.clone();
            let host = entry.host.clone();
            state.overlay.renaming = Some(RenameState::new(name.clone(), name, host));
        }
        Action::RenameInputKey(key) => {
            if let Some(ref mut r) = state.overlay.renaming {
                r.input.input(key);
            }
        }
        Action::RenameConfirm => {
            if let Some(r) = state.overlay.renaming.take() {
                let new_name = textarea_line(&r.input).trim().to_string();
                // Skip no-op renames.
                if !new_name.is_empty() && new_name != r.original_name {
                    fx.rename_session(RenameRequest {
                        old_name: r.original_name,
                        new_name,
                        host: r.host,
                    });
                    fx.refresh_sessions();
                }
            }
        }
        Action::RenameCancel => {
            state.overlay.renaming = None;
        }

        Action::ToggleLayout => {
            state.prefs.layout_mode = match state.prefs.layout_mode {
                LayoutMode::Horizontal => LayoutMode::Vertical,
                LayoutMode::Vertical => LayoutMode::Horizontal,
            };
            fx.resize_pty(true);
            fx.save_config();
        }
        Action::ToggleBorders => {
            state.prefs.show_borders = !state.prefs.show_borders;
            fx.resize_pty(true);
            fx.save_config();
        }
        Action::ToggleTransparentBg => {
            state.prefs.transparent_bg = !state.prefs.transparent_bg;
            fx.save_config();
        }
        Action::SelectTab(tab) => switch_tab(state, &mut fx, tab),
        Action::ToggleSidebarTab => {
            let next = match state.prefs.sidebar_tab {
                SidebarTab::Projects => SidebarTab::Agents,
                SidebarTab::Agents => SidebarTab::Projects,
            };
            switch_tab(state, &mut fx, next);
        }
        Action::ToggleViewMode => {
            state.prefs.view_mode = match state.prefs.view_mode {
                ViewMode::Expanded => ViewMode::Compact,
                ViewMode::Compact => ViewMode::Expanded,
            };
            fx.save_config();
        }
        Action::ToggleSection(key) => {
            // Flip the group's collapsed membership in the active tab's own set
            // (tabs fold independently). The set is stored uniformly even
            // though collapse only matters in Expanded view (Compact/Vertical
            // ignore it), so no view-mode gate is needed. Collapsing the
            // focused row's group doesn't move focus: the highlight is just
            // hidden until expand, and `j`/`k` step out to a visible row.
            let lane_key = crate::system::tmux::lane(key.as_deref());
            if state.agents_tab_active() {
                if !state.collapsed_agent_sections.remove(&lane_key) {
                    state.collapsed_agent_sections.insert(lane_key);
                }
            } else if !state.collapsed_sections.remove(&lane_key) {
                state.collapsed_sections.insert(lane_key);
            }
            fx.save_config();
        }
        Action::Settings(a) => return settings::reduce_settings(state, a),
        Action::Summary(a) => return reduce_summary(state, a),

        Action::TriggerUpgrade | Action::AbortUpgrade => {}

        Action::NewSession(a) => return reduce_new_session(state, a),

        Action::ToggleHelp => {
            state.overlay.show_help = true;
        }
        Action::DismissHelp => {
            state.overlay.show_help = false;
        }

        Action::SetFocusMain => {
            state.focus_mode = FocusMode::Main;
        }
        Action::ToggleFocus => {
            state.focus_mode = match state.focus_mode {
                FocusMode::Main => FocusMode::Sidebar,
                FocusMode::Sidebar => FocusMode::Main,
            };
            if state.focus_mode == FocusMode::Sidebar {
                close_settings_page(state);
            }
        }

        Action::Menu(a) => return menu::reduce_menu(state, a),

        Action::ResizeSidebar(width) => {
            if state.resize_sidebar(width) {
                fx.resize_pty(false);
            }
        }
        Action::ResizeSidebarHeight(height) => {
            if state.resize_sidebar_height(height) {
                fx.resize_pty(false);
            }
        }
        Action::StartDrag => {
            state.dragging_separator = true;
        }
        Action::StopDrag => {
            state.dragging_separator = false;
            fx.save_config();
        }

        Action::Resize(w, h) => {
            state.term_width = w;
            state.term_height = h;
            fx.resize_pty(true);
        }

        Action::ForwardKey(_) | Action::ForwardMouse(_) => {}
        Action::SidebarClickSession(_)
        | Action::NumberKeyJump(_)
        | Action::SwitchToAgentPane(_) => {}

        Action::Quit => {
            fx.quit();
        }

        // Handled entirely in dispatch (needs App-level access to raw
        // keybindings, PTY, etc.).
        Action::ReloadConfig => {}

        // Handled in dispatch (marks the host reconnecting + kicks a
        // refresh round through the worker).
        Action::ReconnectHost { .. } => {}
        // Routed to the owning System at the app layer (`dispatch`), which has
        // the runtime state its effects need; the pure reducer no-ops it.
        Action::SystemButton { .. } => {}

        Action::Pf(a) => return port_forward::reduce_pf(state, a),
        Action::AddRemote(a) => return reduce_add_remote(state, a),

        Action::None => {}
    }

    fx
}

fn reduce_summary(state: &mut AppState, action: SummaryAction) -> SideEffect {
    let mut fx = SideEffect::default();
    match action {
        // Kicked off / torn down in dispatch (needs App-level worker access).
        SummaryAction::Generate => {}
        SummaryAction::Cancel => {}
        SummaryAction::Scroll(delta) => {
            state.last_scroll = std::time::Instant::now();
            state.scroll_summary(delta);
        }
        SummaryAction::OpenPopup => {
            if matches!(
                state.summary.state,
                crate::state::SummaryState::Ready { .. }
            ) {
                state.overlay.summary_popup = true;
                state.summary.popup_scroll = 0;
            }
        }
        SummaryAction::ClosePopup => {
            state.overlay.summary_popup = false;
        }
        SummaryAction::ScrollPopup(delta) => {
            state.scroll_summary_popup(delta);
        }
        SummaryAction::StartDrag => {
            state.summary.dragging = true;
        }
        SummaryAction::Resize(rows) => {
            state.set_summary_height(rows);
        }
        SummaryAction::StopDrag => {
            state.summary.dragging = false;
            fx.save_config();
        }
        SummaryAction::OpenLanguageEditor => {
            state.overlay.summary_lang_input = Some(crate::new_session::make_textarea(
                &state.prefs.summary_language,
            ));
        }
        SummaryAction::LanguageInputKey(key) => {
            if let Some(ref mut ta) = state.overlay.summary_lang_input {
                ta.input(key);
            }
        }
        SummaryAction::LanguageConfirm => {
            if let Some(ta) = state.overlay.summary_lang_input.take() {
                state.prefs.summary_language = textarea_line(&ta).trim().to_string();
                fx.save_config();
            }
        }
        SummaryAction::LanguageCancel => {
            state.overlay.summary_lang_input = None;
        }
    }
    fx
}

fn reduce_new_session(state: &mut AppState, action: NewSessionAction) -> SideEffect {
    let mut fx = SideEffect::default();
    match action {
        NewSessionAction::OpenLocal => {
            fx.open_new_session_picker();
        }
        NewSessionAction::Close => {
            state.overlay.new_session = None;
        }
        NewSessionAction::InputKey(key) => {
            if let Some(ns) = state.overlay.new_session.as_mut() {
                use crate::new_session::PickerFocus;
                match ns.focus {
                    PickerFocus::Name => {
                        ns.name.input(key);
                    }
                    PickerFocus::Dir => {
                        let parent_before = crate::new_session::split_input(ns.input_str())
                            .0
                            .to_string();
                        ns.picker.input.input(key);
                        ns.refilter();
                        let parent_after = crate::new_session::split_input(ns.input_str())
                            .0
                            .to_string();
                        if parent_before != parent_after {
                            fx.reread_new_session_entries();
                        }
                    }
                }
                ns.picker.error = None;
            }
        }
        NewSessionAction::SwitchFocus => {
            if let Some(ns) = state.overlay.new_session.as_mut() {
                ns.focus = match ns.focus {
                    crate::new_session::PickerFocus::Name => crate::new_session::PickerFocus::Dir,
                    crate::new_session::PickerFocus::Dir => crate::new_session::PickerFocus::Name,
                };
                ns.picker.error = None;
            }
        }
        NewSessionAction::DirUp => {
            if let Some(ns) = state.overlay.new_session.as_mut() {
                let parent_before = crate::new_session::split_input(ns.input_str())
                    .0
                    .to_string();
                let mut s = ns.input_str().to_string();
                if s.ends_with('/') && s.len() > 1 {
                    s.pop();
                }
                let new_end = s.rfind('/').map(|i| i + 1).unwrap_or(0);
                s.truncate(new_end);
                ns.set_path(&s);
                let parent_after = crate::new_session::split_input(ns.input_str())
                    .0
                    .to_string();
                if parent_before != parent_after {
                    fx.reread_new_session_entries();
                }
            }
        }
        NewSessionAction::DirEnter => {
            if let Some(ns) = state.overlay.new_session.as_mut() {
                if let Some(&idx) = ns.picker.filtered.get(ns.picker.selected) {
                    let entry = ns.picker.items[idx].clone();
                    let (parent, _leaf) = crate::new_session::split_input(ns.input_str());
                    let new_path = format!("{}{}/", parent, entry);
                    ns.set_path(&new_path);
                    fx.reread_new_session_entries();
                }
            }
        }
        NewSessionAction::Confirm => {
            // Handled at dispatch (needs fs::metadata).
        }
        NewSessionAction::Prev => {
            if let Some(ns) = state.overlay.new_session.as_mut() {
                ns.picker.step(-1);
            }
        }
        NewSessionAction::Next => {
            if let Some(ns) = state.overlay.new_session.as_mut() {
                ns.picker.step(1);
            }
        }
        NewSessionAction::Clear => {
            if let Some(ns) = state.overlay.new_session.as_mut() {
                ns.set_path("");
                fx.reread_new_session_entries();
            }
        }
        NewSessionAction::DeleteSegment => {
            if let Some(ns) = state.overlay.new_session.as_mut() {
                // Trim trailing chars back to (and including) the previous `/`.
                let s = ns.input_str().to_string();
                let mut new_end = s.len();
                while new_end > 0 && !s[..new_end].ends_with('/') {
                    let prev = s[..new_end]
                        .chars()
                        .last()
                        .map(|c| c.len_utf8())
                        .unwrap_or(0);
                    new_end -= prev;
                }
                ns.set_path(&s[..new_end]);
                // Always reread: the user explicitly cleared the segment they
                // were typing and expects a fresh listing of the parent dir.
                fx.reread_new_session_entries();
            }
        }
    }
    fx
}

fn reduce_add_remote(state: &mut AppState, action: AddRemoteAction) -> SideEffect {
    let mut fx = SideEffect::default();
    match action {
        AddRemoteAction::InputKey(key) => {
            if let Some(ar) = state.overlay.add_remote.as_mut() {
                ar.picker.input.input(key);
                ar.refilter();
                ar.picker.error = None;
            }
        }
        AddRemoteAction::Prev => {
            if let Some(ar) = state.overlay.add_remote.as_mut() {
                ar.picker.step(-1);
            }
        }
        AddRemoteAction::Next => {
            if let Some(ar) = state.overlay.add_remote.as_mut() {
                ar.picker.step(1);
            }
        }
        AddRemoteAction::Close => {
            state.overlay.add_remote = None;
        }
        AddRemoteAction::Confirm => {
            // Resolve first (immutable borrow released before we mutate state).
            let chosen = state
                .overlay
                .add_remote
                .as_ref()
                .and_then(|ar| ar.chosen_host());
            let host = match chosen {
                None => {
                    if let Some(ar) = state.overlay.add_remote.as_mut() {
                        ar.picker.error = Some("enter a hostname".into());
                    }
                    return fx;
                }
                Some(h) => h,
            };
            if state.config_remotes.iter().any(|r| r.host == host) {
                if let Some(ar) = state.overlay.add_remote.as_mut() {
                    ar.picker.error = Some("already added".into());
                }
                return fx;
            }
            state.config_remotes.push(crate::config::RemoteConfig {
                host: host.clone(),
                forwards: vec![],
            });
            state.overlay.add_remote = None;
            fx.save_config();
            fx.refresh_sessions();
            fx.add_remote_host(host);
        }
    }
    fx
}

#[cfg(test)]
#[path = "../../../../tests/unit/app/action/reduce.rs"]
mod tests;
