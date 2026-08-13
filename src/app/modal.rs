//! Formal modal rendering. Input and rendering both consume
//! `AppState::active_modal`, so one modal owns the screen and all input even if
//! stale backing state for a lower-priority overlay remains set.

use ratatui::layout::Rect;
use ratatui::Frame;

use crate::action::{
    Action, AddRemoteAction, MenuAction, MountAction, NewSessionAction, PfAction, SettingsAction,
    SummaryAction,
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
    pub new_session_create: Option<Rect>,
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
        Modal::MountPicker => Action::Mount(MountAction::Close),
        Modal::SshSetting => Action::Settings(SettingsAction::SshSettingCancel),
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
                let hits = ui::draw_new_session(
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
                        pinned: ns.pinned_rows(),
                        error: ns.picker.error.as_deref(),
                        lane_title: lane_title.as_deref(),
                    },
                    theme,
                );
                rendered.new_session_dirs = hits.dirs;
                rendered.new_session_create = Some(hits.create);
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
        Modal::MountPicker => {
            if let Some(picker) = state.overlay.mount_picker.as_ref() {
                crate::ui::overlays::mounts::draw_mount_picker(
                    frame,
                    full,
                    picker,
                    &state.section_title(&picker.lane),
                    theme,
                );
            }
        }
        Modal::SshSetting => {
            if let Some(editor) = state.overlay.ssh_setting_editor.as_ref() {
                ui::draw_ssh_setting_editor(
                    frame,
                    main,
                    &ui::SshSettingEditorView {
                        field: editor.field,
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

        // The published confirm button must sit on the footer hint it stands
        // for, so reordering the footer text can't leave a click target
        // pointing at blank space.
        let create = rendered.new_session_create.expect("create hint published");
        let buffer = terminal.backend().buffer();
        let painted: String = (create.x..create.right())
            .map(|x| buffer[(x, create.y)].symbol())
            .collect();
        assert_eq!(painted, "⏎ create");
    }

    /// Scrolling a long listing must not carry `../` off the top: it is the
    /// way out of the directory, so it holds row 0 while the children scroll
    /// under it, and its click target keeps pointing at it.
    #[test]
    fn parent_row_stays_on_screen_and_clickable_when_the_list_is_scrolled() {
        use crate::new_session::{
            make_textarea, with_parent_entry, NewSessionState, PickerFocus, DIRECTORY_VIEW_ROWS,
        };
        use crate::picker::FilterPicker;

        let children: Vec<String> = (0..40).map(|index| format!("child-{index:02}")).collect();
        let mut ns = NewSessionState {
            name: make_textarea("session-1"),
            focus: PickerFocus::Dir,
            picker: FilterPicker::new(with_parent_entry(children)),
            scroll: 0,
            target_lane: Some(crate::system::tmux::TmuxSystem::local_lane()),
        };
        ns.picker.input = make_textarea("~/");
        ns.refilter();
        // Walk to the bottom of the list, the state that used to scroll `..`
        // out of view.
        for _ in 0..39 {
            ns.step_selection(1);
        }
        assert_eq!(ns.entry_at(ns.picker.selected), Some("child-39"));
        assert!(
            ns.scroll >= ns.pinned_rows(),
            "the scroll window must start below the pinned row, got {}",
            ns.scroll
        );

        let mut state = AppState::new(100, 30);
        state.overlay.new_session = Some(ns);

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

        assert_eq!(rendered.new_session_dirs.len(), DIRECTORY_VIEW_ROWS);
        let first = rendered.new_session_dirs[0];
        assert_eq!(first.index, 0, "row 0 must still resolve to the parent row");

        let buffer = terminal.backend().buffer();
        let painted_row = |rect: ratatui::layout::Rect| -> String {
            (rect.x..rect.right())
                .map(|x| buffer[(x, rect.y)].symbol())
                .collect::<String>()
                .trim()
                .to_string()
        };
        assert_eq!(painted_row(first.rect), "../");
        // The row under it is the scroll window's first child, and the last
        // row is the selection we walked to.
        assert!(
            painted_row(rendered.new_session_dirs[1].rect).starts_with("child-"),
            "children must scroll under the pinned row: {:?}",
            painted_row(rendered.new_session_dirs[1].rect)
        );
        let last = rendered.new_session_dirs[DIRECTORY_VIEW_ROWS - 1];
        let last_row = painted_row(last.rect);
        assert!(
            last_row.starts_with("▸ child-39/"),
            "the highlighted child must be the last visible row: {last_row:?}"
        );
        assert!(
            last_row.ends_with('█'),
            "the scrollbar thumb belongs at the bottom of the scrolling part: {last_row:?}"
        );

        // Both footer rows must fit at this popup width, `⎋ cancel` included.
        // `modal_footer` clips silently, so without this the tail of a row can
        // disappear with nothing to show that it did.
        let text = buffer_text(buffer);
        for row in [
            "⏎ create · →← folder · ↑↓ move · ⎋ cancel",
            "click open · right-click create here",
        ] {
            assert!(
                text.contains(row),
                "footer row must fit unclipped: {row:?} missing from {text:?}"
            );
        }
    }

    /// The popup must not resize as the filter narrows, or it shifts under the
    /// cursor on every keystroke. The footer's published rect is the probe:
    /// it is the last row inside the frame, so its `y` is the popup's height.
    #[test]
    fn filtering_does_not_resize_the_popup() {
        use crate::new_session::{make_textarea, with_parent_entry, NewSessionState, PickerFocus};
        use crate::picker::FilterPicker;

        let footer_y_for = |leaf: &str| -> u16 {
            let entries: Vec<String> = (0..20).map(|index| format!("dir-{index:02}")).collect();
            let mut ns = NewSessionState {
                name: make_textarea("session-1"),
                focus: PickerFocus::Dir,
                picker: FilterPicker::new(with_parent_entry(entries)),
                scroll: 0,
                target_lane: Some(crate::system::tmux::TmuxSystem::local_lane()),
            };
            ns.picker.input = make_textarea(&format!("~/{leaf}"));
            ns.refilter();

            let mut state = AppState::new(100, 30);
            state.overlay.new_session = Some(ns);
            let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
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
            rendered.new_session_create.expect("footer published").y
        };

        // 21 rows, then 1 (`dir-07` alone), then 0 matches.
        let full = footer_y_for("");
        assert_eq!(full, footer_y_for("dir-0"), "narrowing must not resize");
        assert_eq!(full, footer_y_for("dir-07"), "one match must not resize");
        assert_eq!(full, footer_y_for("nothing"), "no matches must not resize");
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
