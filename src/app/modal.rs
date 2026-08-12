//! Formal modal rendering. Input and rendering both consume
//! `AppState::active_modal`, so one modal owns the screen and all input even if
//! stale backing state for a lower-priority overlay remains set.

use ratatui::layout::Rect;
use ratatui::Frame;

use crate::action::{
    Action, AddRemoteAction, MenuAction, NewSessionAction, PfAction, SettingsAction, SummaryAction,
};
use crate::geometry::KillConfirmHits;
use crate::overlay::Modal;
use crate::state::{AppState, LayoutMode};
use crate::theme::{Theme, THEMES};
use crate::ui;

#[derive(Default)]
pub(super) struct RenderedModal {
    pub summary_popup_max_scroll: usize,
    pub kill_hits: Option<KillConfirmHits>,
    pub new_session_dirs: Vec<crate::geometry::ListItemHit>,
}

/// One close/cancel policy for every formal modal. Keyboard routing handles
/// Esc before modal-specific keys, so forms cannot drift on whether it closes
/// the surface or only cancels their nested edit mode.
pub(super) fn close_action(modal: Modal, state: &AppState) -> Action {
    match modal {
        Modal::SummaryPopup => Action::Summary(SummaryAction::ClosePopup),
        Modal::NewSession => Action::NewSession(NewSessionAction::Close),
        Modal::AddRemote => Action::AddRemote(AddRemoteAction::Close),
        Modal::Rename => Action::RenameCancel,
        Modal::ContextMenu => Action::Menu(MenuAction::Dismiss),
        Modal::PortForward => {
            let action = if state
                .overlay
                .port_forward
                .as_ref()
                .is_some_and(|overlay| overlay.add_form.is_some())
            {
                PfAction::AddCancel
            } else {
                PfAction::Close
            };
            Action::Pf(action)
        }
        Modal::ThemePicker => Action::Settings(SettingsAction::CloseThemePicker),
        Modal::KeybindingsView => Action::Settings(SettingsAction::CloseKeybindingsView),
        Modal::ExcludeEditor => {
            let action = if state
                .overlay
                .exclude_editor
                .as_ref()
                .is_some_and(|editor| editor.adding)
            {
                SettingsAction::ExcludeCancelAdd
            } else {
                SettingsAction::ExcludeClose
            };
            Action::Settings(action)
        }
        Modal::SummaryLang => Action::Summary(SummaryAction::LanguageCancel),
        Modal::Help => Action::DismissHelp,
        Modal::ConfirmKill => Action::CancelKill,
    }
}

pub(super) fn draw_active_modal(
    frame: &mut Frame,
    state: &AppState,
    full: Rect,
    main: Rect,
    layout_mode: LayoutMode,
    theme: &Theme,
) -> RenderedModal {
    let Some(modal) = state.active_modal() else {
        return RenderedModal::default();
    };

    let mut rendered = RenderedModal::default();

    match modal {
        Modal::SummaryPopup => {
            if let crate::summary_card::SummaryState::Ready { text, .. } = &state.summary.state {
                rendered.summary_popup_max_scroll =
                    ui::draw_summary_popup(frame, full, text, state.summary.popup_scroll, theme);
            }
        }
        Modal::NewSession => {
            if let Some(ns) = state.overlay.new_session.as_ref() {
                let lane_title = ns
                    .target_lane
                    .as_ref()
                    .filter(|lane| !state.is_primary_lane(lane))
                    .map(|lane| state.section_title(lane));
                rendered.new_session_dirs = ui::draw_new_session(
                    frame,
                    full,
                    &ui::NewSessionView {
                        name: &ns.name,
                        focus_name: matches!(ns.focus, crate::new_session::PickerFocus::Name),
                        input: &ns.picker.input,
                        entries: &ns.picker.items,
                        filtered: &ns.picker.filtered,
                        selected: ns.picker.selected,
                        scroll: ns.scroll,
                        error: ns.picker.error.as_deref(),
                        lane_title: lane_title.as_deref(),
                    },
                    theme,
                );
            }
        }
        Modal::AddRemote => {
            if let Some(picker) = state.overlay.add_remote.as_ref() {
                ui::draw_add_remote(frame, full, picker, theme);
            }
        }
        Modal::Rename => {
            // Horizontal layout renders rename in the sidebar. The vertical
            // tab bar cannot hold the editor, so it uses a centered surface.
            if layout_mode == LayoutMode::Vertical {
                if let Some(rename) = state.overlay.renaming.as_ref() {
                    ui::draw_rename_popup(frame, full, theme, &rename.input);
                }
            }
        }
        Modal::ContextMenu => {
            if let Some(menu) = state.overlay.context_menu.as_ref() {
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
        }
        Modal::PortForward => {
            if let Some(overlay) = state.overlay.port_forward.as_ref() {
                let lane_title = state.section_title(&overlay.lane);
                let forwards = crate::app::ssh::config_adapter::remote_for_lane(
                    &state.config_remotes,
                    &overlay.lane,
                )
                .map_or(&[][..], |remote| remote.forwards.as_slice());
                crate::ui::overlays::port_forward::draw_port_forward(
                    frame,
                    full,
                    overlay,
                    &lane_title,
                    forwards,
                    theme,
                );
            }
        }
        Modal::ThemePicker => {
            let theme_names: Vec<&str> = THEMES.iter().map(|candidate| candidate.name).collect();
            ui::draw_theme_picker(
                frame,
                main,
                &theme_names,
                state.settings.theme_picker_selected,
                theme,
            );
        }
        Modal::KeybindingsView => ui::draw_keybindings_view(
            frame,
            main,
            &state.keybindings,
            state.settings.keybindings_view_scroll,
            theme,
        ),
        Modal::ExcludeEditor => {
            if let Some(editor) = state.overlay.exclude_editor.as_ref() {
                ui::draw_exclude_editor(
                    frame,
                    main,
                    &ui::ExcludeEditorView {
                        patterns: &state.prefs.exclude_patterns,
                        selected: editor.selected,
                        adding: editor.adding,
                        input: &editor.input,
                        error: editor.error.as_deref(),
                    },
                    theme,
                );
            }
        }
        Modal::SummaryLang => {
            if let Some(input) = state.overlay.summary_lang_input.as_ref() {
                ui::draw_summary_language_editor(frame, main, input, theme);
            }
        }
        Modal::Help => {
            if layout_mode == LayoutMode::Vertical {
                ui::draw_help_popup(frame, full, theme, &state.keybindings);
            }
        }
        Modal::ConfirmKill => {
            if layout_mode == LayoutMode::Vertical {
                if let Some(name) = state.confirm_kill_name() {
                    rendered.kill_hits = ui::draw_confirm_kill_popup(frame, full, theme, &name);
                }
            }
        }
    }

    rendered
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::Terminal;

    fn buffer_text(buffer: &Buffer) -> String {
        (0..buffer.area.height)
            .flat_map(|y| (0..buffer.area.width).map(move |x| buffer[(x, y)].symbol()))
            .collect()
    }

    #[test]
    fn renderer_draws_only_the_modal_that_owns_input() {
        use crate::forwards::PortForwardOverlay;
        use crate::new_session::{make_textarea, NewSessionState, PickerFocus};
        use crate::picker::FilterPicker;

        let mut state = AppState::new(100, 30);
        state.overlay.new_session = Some(NewSessionState {
            name: make_textarea("session-1"),
            focus: PickerFocus::Name,
            picker: FilterPicker::new(vec!["~/project".to_string()]),
            scroll: 0,
            target_lane: Some(crate::system::tmux::TmuxSystem::local_lane()),
        });
        // Simulate stale lower-priority state. Before the unified renderer,
        // Port Forward was painted later even though New Session got input.
        state.overlay.port_forward = Some(PortForwardOverlay {
            lane: crate::system::tmux::TmuxSystem::host_lane("stale-host"),
            selected: 0,
            add_form: None,
            status: None,
        });

        assert_eq!(state.active_modal(), Some(Modal::NewSession));

        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut rendered = RenderedModal::default();
        terminal
            .draw(|frame| {
                let area = frame.area();
                rendered = draw_active_modal(
                    frame,
                    &state,
                    area,
                    area,
                    LayoutMode::Horizontal,
                    &THEMES[0],
                );
            })
            .unwrap();

        let text = buffer_text(terminal.backend().buffer());
        assert!(
            text.contains("New session"),
            "active modal missing: {text:?}"
        );
        assert!(
            !text.contains("Port Forward"),
            "lower-priority modal painted over active modal: {text:?}"
        );
        assert_eq!(rendered.new_session_dirs.len(), 1);
        assert_eq!(rendered.new_session_dirs[0].index, 0);
    }

    #[test]
    fn vertical_confirm_is_visible_and_owns_click_regions() {
        use crate::state::{SessionEntry, SessionEntryKind};

        let mut state = AppState::new(100, 30);
        state.entries = vec![SessionEntry {
            lane: crate::system::tmux::TmuxSystem::local_lane(),
            name: "victim".to_string(),
            dir: "/tmp".to_string(),
            kind: SessionEntryKind::Live { is_current: false },
        }];
        state.overlay.confirm_kill = true;

        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut rendered = RenderedModal::default();
        terminal
            .draw(|frame| {
                let area = frame.area();
                rendered =
                    draw_active_modal(frame, &state, area, area, LayoutMode::Vertical, &THEMES[0]);
            })
            .unwrap();

        let text = buffer_text(terminal.backend().buffer());
        assert!(
            text.contains("Close victim"),
            "confirm modal missing: {text:?}"
        );
        assert!(
            rendered.kill_hits.is_some(),
            "vertical confirm must publish modal button hit regions"
        );
    }
}
