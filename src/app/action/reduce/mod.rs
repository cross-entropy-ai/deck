use crate::effects::{Effect, KillRequest, RenameRequest, SideEffect};
use crate::new_session::textarea_line;
use crate::overlay::RenameState;
use crate::state::{AppState, FocusMode, LayoutMode, MainView, SidebarTab, ViewMode};

use super::{
    Action, AddRemoteAction, MenuAction, MountAction, NewSessionAction, PfAction, SettingsAction,
    SummaryAction,
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
    state.settings.reset_pages();
    state.settings.theme_picker_open = false;
    state.settings.keybindings_view_open = false;
    state.overlay.exclude_editor = None;
    state.overlay.summary_lang_input = None;
    state.overlay.ssh_setting_editor = None;
}

/// Activate the focused live session through one lane-qualified identity.
/// Synthetic placeholder rows have no session identity and retain their
/// temporary presentation-only effect until attachments become lane-keyed.
fn fill_switch_effect(state: &AppState, fx: &mut SideEffect) -> bool {
    let Some(target) = state.focus_target() else {
        return false;
    };
    match state.entry_at(target) {
        Some(entry) if entry.is_attachable() => {
            if state.session_capabilities(&entry.lane).activate {
                fx.push(Effect::ActivateSession(entry.id()));
                true
            } else {
                false
            }
        }
        Some(entry) => {
            fx.push(Effect::ShowLanePlaceholder(entry.lane.clone()));
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
    state
        .focused_agent()
        .map(|target| fx.push(Effect::SwitchAgentPane(target)))
        .is_some()
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

/// Move the focused project directly to `target`, preserving lane boundaries
/// and emitting one persistence effect for the completed gesture. Keyboard
/// adjacent moves and mouse drag drops share this path.
fn reorder_session_to(state: &mut AppState, target: usize, fx: &mut SideEffect) {
    if state.agents_tab_active() {
        return;
    }
    let source = state.focused;
    if source == target || target >= state.entries.len() {
        return;
    }

    let Some(entry) = state.entries.get(source) else {
        return;
    };
    if !state.lane_capabilities(&entry.lane).reorder_sessions {
        return;
    }
    let lane = entry.lane.clone();
    if !state.is_primary_entry(entry) {
        if !entry.is_attachable()
            || !state.entries[target].is_attachable()
            || state.entries[target].lane != lane
        {
            return;
        }

        let moved = state.entries.remove(source);
        state.entries.insert(target, moved);
        state.focused = target;
        fx.push(Effect::SaveSessionOrder(lane));
        return;
    }

    // Local rows remain ahead of every remote lane. Resolve both names through
    // `session_order` so the persisted tmux ranks and visible entries move as
    // one, even if a refresh previously changed their raw listing order.
    let Some(target_entry) = state.entries.get(target) else {
        return;
    };
    if !state.is_primary_entry(target_entry) {
        return;
    }
    let name = entry.name.clone();
    let target_name = target_entry.name.clone();
    let Some(source_pos) = state.session_order.iter().position(|n| n == &name) else {
        return;
    };
    let Some(target_pos) = state.session_order.iter().position(|n| n == &target_name) else {
        return;
    };

    let moved = state.session_order.remove(source_pos);
    state.session_order.insert(target_pos, moved);
    state.apply_order();
    state.clamp_projects_focus();
    if let Some(new_focused) = state
        .entries
        .iter()
        .position(|e| state.is_primary_entry(e) && e.name == name)
    {
        state.focused = new_focused;
    }
    // Persist once, on drop (or once per keyboard move), so a pointer crossing
    // several rows never spams tmux or a remote SSH connection.
    fx.push(Effect::SaveSessionOrder(lane));
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
            // number-key shortcuts resolve their visible slot to the same flat
            // index. Reject a collapsed target here too, so a stale mapper or
            // synthetic action cannot move the cursor onto a hidden row.
            if idx < state.focusable_count() && !state.is_focus_collapsed(idx) {
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
            // Same policy the context menu uses to grey "Close": no closing a
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
            let lane = entry.lane.clone();
            if state.is_primary_entry(entry) {
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

                fx.push(Effect::KillSession(KillRequest {
                    name,
                    lane,
                    switch_to,
                }));
                fx.refresh_sessions();
            } else {
                let name = entry.name.clone();
                let lane_indices: Vec<usize> = state
                    .entries
                    .iter()
                    .enumerate()
                    .filter_map(|(idx, candidate)| {
                        (candidate.lane == lane && candidate.is_attachable()).then_some(idx)
                    })
                    .collect();
                let lane_pos = lane_indices.iter().position(|idx| *idx == state.focused);
                let alternative_idx = lane_pos.and_then(|pos| {
                    lane_indices
                        .get(pos + 1)
                        .or_else(|| pos.checked_sub(1).and_then(|prev| lane_indices.get(prev)))
                        .copied()
                });
                let switch_to = alternative_idx
                    .and_then(|idx| state.entries.get(idx))
                    .map(|candidate| candidate.name.clone());
                if let Some(idx) = alternative_idx {
                    state.focused = idx;
                }
                fx.push(Effect::KillSession(KillRequest {
                    name,
                    lane,
                    switch_to,
                }));
                fx.refresh_sessions();
            }
        }
        Action::CancelKill => {
            state.overlay.confirm_kill = false;
        }
        Action::RemoveLane(lane) => {
            // Configuration ownership belongs to the lane runtime. App applies
            // the provider's typed result and reconciles state after success.
            fx.push(Effect::RemoveLane(lane));
        }
        Action::ReorderSession(direction) => {
            if state.agents_tab_active() {
                return fx;
            }
            let idx = state.focused;
            let target = idx as i32 + direction;
            if target >= 0 {
                reorder_session_to(state, target as usize, &mut fx);
            }
        }
        Action::ReorderSessionTo(target) => reorder_session_to(state, target, &mut fx),
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
            if !entry.is_attachable() || !state.session_capabilities(&entry.lane).rename {
                return fx;
            }
            let name = entry.name.clone();
            let lane = entry.lane.clone();
            state.overlay.renaming = Some(RenameState::new_with_lane(name.clone(), name, lane));
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
                if new_name == r.original_name {
                    return fx;
                }

                let existing = state
                    .entries
                    .iter()
                    .filter(|entry| {
                        entry.is_attachable()
                            && entry.lane == r.lane
                            && entry.name != r.original_name
                    })
                    .map(|entry| entry.name.as_str());
                if let Some(error) =
                    crate::new_session::validate_unique_session_name(&new_name, existing)
                {
                    state.show_warning(error);
                    state.overlay.renaming = Some(r);
                    return fx;
                }

                fx.push(Effect::RenameSession(RenameRequest {
                    old_name: r.original_name,
                    new_name,
                    lane: r.lane,
                }));
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
        Action::ToggleSidebar => {
            state.prefs.sidebar_collapsed = !state.prefs.sidebar_collapsed;
            if state.prefs.sidebar_collapsed {
                state.focus_mode = FocusMode::Main;
                state.dragging_separator = false;
                state.project_drag.cancel();
            } else if state.prefs.sidebar_tab == SidebarTab::Agents {
                fx.refresh_sessions();
            }
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
            if state.agents_tab_active() {
                if !state.collapsed_agent_sections.remove(&key) {
                    state.collapsed_agent_sections.insert(key);
                }
            } else if !state.collapsed_sections.remove(&key) {
                state.collapsed_sections.insert(key);
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
            if state.prefs.sidebar_collapsed {
                state.prefs.sidebar_collapsed = false;
                state.focus_mode = FocusMode::Sidebar;
                close_settings_page(state);
                fx.resize_pty(true);
                fx.save_config();
                if state.prefs.sidebar_tab == SidebarTab::Agents {
                    fx.refresh_sessions();
                }
                return fx;
            }
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
        Action::StartProjectDrag(row) => {
            if !state.agents_tab_active() {
                if let Some(idx) = state.start_project_drag(row, std::time::Instant::now()) {
                    state.focused = idx;
                }
            }
        }
        Action::UpdateProjectDrag(row) => {
            if let Some(idx) = state.update_project_drag(row) {
                state.focused = idx;
            }
        }
        // App::dispatch consumes the library's RowMove so an unchanged drag
        // can retain ordinary click-to-switch behavior.
        Action::FinishProjectDrag => {}

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
            fx.push(Effect::Quit);
        }

        // Handled entirely in dispatch (needs App-level access to raw
        // keybindings, PTY, etc.).
        Action::ReloadConfig => {}

        Action::InvokeLane {
            lane,
            action,
            anchor,
        } => fx.push(Effect::InvokeLaneAction {
            lane,
            action,
            anchor,
        }),

        Action::Pf(a) => return port_forward::reduce_pf(state, a),
        Action::AddRemote(a) => return reduce_add_remote(state, a),
        Action::Mount(a) => return reduce_mount(state, a),

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
                crate::summary_card::SummaryState::Ready { .. }
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

/// `s` truncated to (and including) its last `/`, or empty when it has none.
/// `rfind` returns a char boundary, so no manual UTF-8 walk is needed.
fn to_parent(s: &str) -> &str {
    &s[..s.rfind('/').map_or(0, |i| i + 1)]
}

fn reduce_new_session(state: &mut AppState, action: NewSessionAction) -> SideEffect {
    let mut fx = SideEffect::default();
    match action {
        NewSessionAction::OpenLocal => {
            if let Some(lane) = state.primary_lane() {
                fx.push(Effect::OpenNewSessionPicker(lane.clone()));
            }
            return fx;
        }
        NewSessionAction::Close => {
            state.overlay.new_session = None;
            return fx;
        }
        _ => {}
    }
    let Some(ns) = state.overlay.new_session.as_mut() else {
        return fx;
    };
    match action {
        // Handled above, before the overlay guard.
        NewSessionAction::OpenLocal | NewSessionAction::Close => {}
        NewSessionAction::InputKey(key) => {
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
        NewSessionAction::SwitchFocus => {
            ns.focus = match ns.focus {
                crate::new_session::PickerFocus::Name => crate::new_session::PickerFocus::Dir,
                crate::new_session::PickerFocus::Dir => crate::new_session::PickerFocus::Name,
            };
            ns.picker.error = None;
        }
        NewSessionAction::DirUp => {
            let parent_before = crate::new_session::split_input(ns.input_str())
                .0
                .to_string();
            let mut s = ns.input_str().to_string();
            if s.ends_with('/') && s.len() > 1 {
                s.pop();
            }
            ns.set_path(to_parent(&s));
            let parent_after = crate::new_session::split_input(ns.input_str())
                .0
                .to_string();
            if parent_before != parent_after {
                fx.reread_new_session_entries();
            }
        }
        NewSessionAction::DirEnter => {
            if let Some(&idx) = ns.picker.filtered.get(ns.picker.selected) {
                let entry = ns.picker.items[idx].clone();
                let (parent, _leaf) = crate::new_session::split_input(ns.input_str());
                let new_path = if entry == ".." {
                    crate::new_session::parent_directory(parent)
                } else {
                    format!("{}{}/", parent, entry)
                };
                ns.set_path(&new_path);
                fx.reread_new_session_entries();
            }
        }
        NewSessionAction::Confirm => {
            // Handled at dispatch (needs fs::metadata).
        }
        NewSessionAction::Prev => ns.step_selection(-1),
        NewSessionAction::Next => ns.step_selection(1),
        NewSessionAction::Select(index) => {
            if index < ns.picker.filtered.len() {
                ns.picker.selected = index;
                ns.focus = crate::new_session::PickerFocus::Dir;
                ns.picker.error = None;
                ns.keep_selection_visible();
            }
        }
        NewSessionAction::Clear => {
            ns.set_path("");
            fx.reread_new_session_entries();
        }
        NewSessionAction::DeleteSegment => {
            // Trim trailing chars back to (and including) the previous `/`.
            let s = ns.input_str().to_string();
            ns.set_path(to_parent(&s));
            // Always reread: the user explicitly cleared the segment they
            // were typing and expects a fresh listing of the parent dir.
            fx.reread_new_session_entries();
        }
    }
    fx
}

/// The mount picker. Mirrors `reduce_add_remote`, with two additions: worker
/// answers arrive as actions (so a stale `generation` can be dropped), and a
/// candidate that declared `needs_activation` takes a second Enter, because
/// mounting it changes something outside Deck.
fn reduce_mount(state: &mut AppState, action: MountAction) -> SideEffect {
    let mut fx = SideEffect::default();
    match action {
        MountAction::Open(lane) => {
            if !state.lane_capabilities(&lane).mounts {
                return fx;
            }
            // A fresh generation retires any probe still in flight for a
            // previous picker.
            state.mount_generation = state.mount_generation.wrapping_add(1);
            let generation = state.mount_generation;
            state.overlay.context_menu = None;
            state.overlay.mount_picker = Some(crate::overlay::MountPickerState::new(
                lane.clone(),
                generation,
            ));
            fx.push(Effect::DiscoverMounts { lane, generation });
        }
        MountAction::InputKey(_) | MountAction::Prev | MountAction::Next => {
            let Some(picker) = state.overlay.mount_picker.as_mut() else {
                return fx;
            };
            // Any navigation abandons a pending confirmation rather than
            // carrying it onto whatever is highlighted next.
            picker.confirming = None;
            match action {
                MountAction::InputKey(key) => {
                    picker.picker.input.input(key);
                    picker.refilter();
                    picker.picker.error = None;
                }
                MountAction::Prev => picker.picker.step(-1),
                _ => picker.picker.step(1),
            }
        }
        MountAction::Close => {
            state.overlay.mount_picker = None;
        }
        MountAction::Confirm => {
            let Some(picker) = state.overlay.mount_picker.as_mut() else {
                return fx;
            };
            if picker.busy.is_some() {
                return fx;
            }
            // Second Enter on a candidate we already asked about.
            if let Some(pending) = picker.confirming.take() {
                picker.busy = Some(crate::overlay::MountBusy::Activating);
                fx.push(Effect::ActivateMount {
                    lane: picker.lane.clone(),
                    generation: picker.generation,
                    candidate: pending.id,
                });
                return fx;
            }
            let Some(candidate) = picker.selected().cloned() else {
                picker.picker.error = Some("nothing to mount".into());
                return fx;
            };
            if candidate.needs_activation {
                picker.confirming = Some(candidate);
                return fx;
            }
            let lane = picker.lane.clone();
            state.overlay.mount_picker = None;
            fx.push(Effect::MountLane {
                lane,
                candidate: candidate.id,
            });
        }
        MountAction::Discovered {
            lane,
            generation,
            result,
        } => {
            let Some(picker) = state.overlay.mount_picker.as_mut() else {
                return fx;
            };
            if picker.generation != generation || picker.lane != lane {
                return fx;
            }
            match result {
                Ok(candidates) => picker.set_candidates(candidates),
                Err(error) => {
                    picker.busy = None;
                    picker.picker.error = Some(error);
                }
            }
        }
        MountAction::Activated {
            lane,
            generation,
            candidate,
            result,
        } => {
            let Some(picker) = state.overlay.mount_picker.as_mut() else {
                return fx;
            };
            if picker.generation != generation || picker.lane != lane {
                return fx;
            }
            picker.busy = None;
            match result {
                Ok(()) => {
                    state.overlay.mount_picker = None;
                    fx.push(Effect::MountLane { lane, candidate });
                }
                Err(error) => picker.picker.error = Some(error),
            }
        }
    }
    fx
}

fn reduce_add_remote(state: &mut AppState, action: AddRemoteAction) -> SideEffect {
    let mut fx = SideEffect::default();
    match action {
        // The input/navigation arms all edit the open picker; one guard for all.
        AddRemoteAction::InputKey(_) | AddRemoteAction::Prev | AddRemoteAction::Next => {
            let Some(ar) = state.overlay.add_remote.as_mut() else {
                return fx;
            };
            match action {
                AddRemoteAction::InputKey(key) => {
                    ar.picker.input.input(key);
                    ar.refilter();
                    ar.picker.error = None;
                }
                AddRemoteAction::Prev => ar.picker.step(-1),
                _ => ar.picker.step(1),
            }
        }
        AddRemoteAction::Close => {
            state.overlay.add_remote = None;
        }
        AddRemoteAction::Confirm => {
            let request = state.overlay.add_remote.as_ref().and_then(|picker| {
                picker
                    .chosen_host()
                    .map(|candidate| (picker.owner.clone(), candidate))
            });
            match request {
                Some((owner, candidate)) => {
                    fx.push(Effect::AddConfiguredLane { owner, candidate });
                }
                None => {
                    if let Some(ar) = state.overlay.add_remote.as_mut() {
                        ar.picker.error = Some("enter a lane identifier".into());
                    }
                }
            }
        }
    }
    fx
}

#[cfg(test)]
#[path = "../../../../tests/unit/app/action/reduce.rs"]
mod tests;
