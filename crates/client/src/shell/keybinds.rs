//! The key bindings in the window (ADR 0777): what each action does, and the Keybinds section
//! of the settings, where the person records the keys of each action.

use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, Modifiers, ParentElement, Styled, Window,
    div,
};
use junto_client_core::global_keys::{Key, KeysState, Permission, ReadError};
use junto_client_core::keybinds::{ActionGroup, KeyAction};

use super::{Open, Shell};
use crate::state::KeybindChange;
use crate::state::view::ShellView;
use crate::text;
use crate::theme::{Theme, u};
use crate::ui::{Button, ButtonKind, StyleExt, Toggle};

/// The keys that a window holds and what its settings record.
#[derive(Debug, Default)]
pub(crate) struct WindowKeys {
    pub modifiers: Modifiers,
    /// The last key other than a modifier that went down and not up yet.
    pub down: Option<Key>,
    /// The window holds the keys of push to talk, and of push to mute.
    pub talks: bool,
    pub mutes: bool,
    /// The settings wait for the keys of this action.
    pub recording: Option<KeyAction>,
    pub recording_release: bool,
    /// Why the last recorded keys were refused.
    pub refusal: Option<String>,
    /// The keystroke whose binding the window performed before the actions of the focused
    /// element; the key-down listener of the window then leaves it.
    pub intercepted: Option<gpui::Keystroke>,
}

/// The name of a control of the row of an action, which a window test finds: one name per
/// control and action, made once.
fn selector(control: &'static str, action: KeyAction) -> &'static str {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static NAMES: OnceLock<Mutex<HashMap<(&'static str, KeyAction), &'static str>>> =
        OnceLock::new();
    let mut names = NAMES
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    names.entry((control, action)).or_insert_with(|| {
        Box::leak(format!("keybind-{control}-{}", action.name()).into_boxed_str())
    })
}

/// The neighbour of the entry for which `is_current` holds, one step in the direction, around
/// the end. The first entry when none is current.
fn step<T: Copy>(entries: &[T], is_current: impl Fn(&T) -> bool, forward: bool) -> Option<T> {
    let count = entries.len();
    if count == 0 {
        return None;
    }
    let next = match entries.iter().position(is_current) {
        Some(at) if forward => (at + 1) % count,
        Some(at) => (at + count - 1) % count,
        None => 0,
    };
    entries.get(next).copied()
}

impl Shell {
    /// Does the action of a key binding in this window.
    pub(crate) fn perform_key_action(
        &mut self,
        action: KeyAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.state.read(cx).recording_keybind() {
            return;
        }
        if self
            .state
            .update(cx, |state, cx| state.perform_call_action(action, cx))
        {
            return;
        }
        let view = self.state.read(cx).view();
        match action {
            KeyAction::ShareScreen => {
                if view.call.as_ref().is_some_and(|call| call.can_share) {
                    self.open(Open::SharePicker, window, cx);
                }
            }
            KeyAction::ShowCall => self.state.update(cx, |state, cx| state.show_call(cx)),
            // The panel of the call window, the panel over the content of a narrow main
            // window, or the panel beside the stage of a wide one.
            KeyAction::ToggleCallPanel => {
                if view.call.is_some() {
                    let theme = crate::theme::theme(cx);
                    if self.detached || super::logical_width(window) < theme.metrics.narrow_below {
                        self.call_panel_open = !self.call_panel_open;
                    } else {
                        self.call_panel_hidden = !self.call_panel_hidden;
                    }
                    cx.notify();
                }
            }
            KeyAction::Back => {
                self.state.update(cx, |state, cx| state.go_back(cx));
            }
            KeyAction::Forward => {
                self.state.update(cx, |state, cx| state.go_forward(cx));
            }
            KeyAction::OpenDirectMessages => {
                let newest = view
                    .lists()
                    .and_then(|lists| lists.directs.first())
                    .map(|row| row.id);
                if let (Some(server), Some(conversation_id)) = (&view.selected, newest) {
                    let server_id = server.id;
                    self.state.update(cx, |state, cx| {
                        state.select_conversation(server_id, conversation_id, cx);
                    });
                }
            }
            KeyAction::NewDirectMessage => {
                if let Some(server) = &view.selected {
                    let server_id = server.id;
                    self.open_new_direct(server_id, window, cx);
                }
            }
            KeyAction::PreviousServer | KeyAction::NextServer => {
                let ids: Vec<_> = view.servers.iter().map(|row| row.id).collect();
                let current = view
                    .servers
                    .iter()
                    .find(|row| row.selected)
                    .map(|row| row.id);
                let forward = action == KeyAction::NextServer;
                if let Some(next) = step(&ids, |id| Some(*id) == current, forward) {
                    self.state
                        .update(cx, |state, cx| state.select_server(next, cx));
                }
            }
            KeyAction::PreviousWorkspace | KeyAction::NextWorkspace => {
                let Some(row) = view.servers.iter().find(|row| row.selected) else {
                    return;
                };
                let ids: Vec<_> = row
                    .workspaces
                    .iter()
                    .map(|workspace| workspace.id)
                    .collect();
                let current = row
                    .workspaces
                    .iter()
                    .find(|workspace| workspace.selected)
                    .map(|workspace| workspace.id);
                let forward = action == KeyAction::NextWorkspace;
                if let Some(next) = step(&ids, |id| Some(*id) == current, forward) {
                    let server_id = row.id;
                    self.state
                        .update(cx, |state, cx| state.select_workspace(server_id, next, cx));
                }
            }
            KeyAction::PreviousChannel
            | KeyAction::NextChannel
            | KeyAction::PreviousUnreadChannel
            | KeyAction::NextUnreadChannel => self.step_channel(&view, action, cx),
            KeyAction::ToggleServerBook => self.state.update(cx, |state, cx| {
                let collapsed = state.settings().server_book_collapsed;
                state.set_server_book_collapsed(!collapsed, cx);
            }),
            KeyAction::Search => self.focus_search(window, cx),
            KeyAction::Notices => self.toggle_notices(cx),
            KeyAction::JumpToNewest => self.go_to_end(super::Pane::Main, window, cx),
            KeyAction::MembersList => {
                self.members_open = !self.members_open;
                cx.notify();
            }
            // As the pins button of the header: a second press closes them.
            KeyAction::PinnedMessages => {
                let shown = view.shown();
                if view
                    .workspace()
                    .is_some_and(|workspace| workspace.pins.is_some())
                {
                    self.state.update(cx, |state, cx| state.close_pins(cx));
                } else if let (Some(server_id), Some(channel_id)) = (shown.server, shown.channel) {
                    self.open_pins(server_id, channel_id, cx);
                }
            }
            KeyAction::MarkChannelRead => self.mark_shown_as_read(cx),
            KeyAction::MarkServerRead => {
                if let Some(server) = &view.selected {
                    let server_id = server.id;
                    self.state
                        .update(cx, |state, _| state.mark_server_read(server_id, None));
                }
            }
            KeyAction::ReplyToNewest => {
                if let Some(message_id) = self.newest_message(super::Pane::Main) {
                    self.reply_to_message(super::Pane::Main, message_id, window, cx);
                }
            }
            // A draft in the composer is not replaced.
            KeyAction::EditLastMessage => {
                if self
                    .pen_mut(super::Pane::Main)
                    .input
                    .read(cx)
                    .value()
                    .is_empty()
                {
                    self.edit_last_own_message(super::Pane::Main, window, cx);
                }
            }
            KeyAction::UploadFile => self.choose_files(super::Pane::Main, cx),
            KeyAction::OpenSettings => self.open(Open::Settings, window, cx),
            KeyAction::OpenAdministration => {
                self.open_server_admin(crate::state::admin::AdminPage::Workspaces, window, cx);
            }
            KeyAction::OpenKeybinds => {
                self.open(Open::Settings, window, cx);
                self.open_settings_section(super::settings::SettingsSection::Keybinds, window, cx);
            }
            // The call actions are above; push to talk and push to mute are held keys.
            KeyAction::ToggleMute
            | KeyAction::ToggleSoundOff
            | KeyAction::PushToTalk
            | KeyAction::PushToMute
            | KeyAction::ToggleCamera
            | KeyAction::LeaveCall
            | KeyAction::AnswerCall
            | KeyAction::DeclineCall => {}
        }
    }

    /// The newest message that the list of the pane holds.
    fn newest_message(
        &self,
        pane: super::Pane,
    ) -> Option<junto_client_core::protocol::ids::MessageId> {
        self.pane_feed(pane)?
            .rows
            .iter()
            .rev()
            .find_map(|row| match row {
                crate::state::feed::FeedRow::Message(row) => Some(row.message.id),
                _ => None,
            })
    }

    /// The previous or the next channel of the shown workspace, or of those that hold
    /// something unread, in the order of the channel list.
    fn step_channel(&mut self, view: &ShellView, action: KeyAction, cx: &mut Context<Self>) {
        let Some(workspace) = view.workspace() else {
            return;
        };
        let unread = matches!(
            action,
            KeyAction::PreviousUnreadChannel | KeyAction::NextUnreadChannel
        );
        let rows: Vec<_> = workspace
            .sections
            .iter()
            .flat_map(|section| &section.rows)
            .filter(|row| !unread || row.selected || row.unread.unread())
            .map(|row| (row.id, row.selected))
            .collect();
        let forward = matches!(
            action,
            KeyAction::NextChannel | KeyAction::NextUnreadChannel
        );
        if let Some((channel_id, false)) = step(&rows, |(_, selected)| *selected, forward) {
            let workspace_id = workspace.id;
            self.state.update(cx, |state, cx| {
                state.select_channel(workspace_id, channel_id, cx);
            });
        }
    }

    /// The Keybinds section of the settings (ADR 0777): how the keys reach Junto, and each
    /// action of each group with its keys.
    pub(super) fn render_keybinds_section(
        &mut self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut parts =
            vec![super::settings::group_label(text::KEYBINDS, theme).into_any_element()];
        parts.push(super::settings::help(text::KEYBINDS_HELP, theme).into_any_element());
        parts.extend(self.render_keys_reach(theme, cx));
        let changed = KeyAction::ALL
            .into_iter()
            .any(|action| self.state.read(cx).keybinds().is_changed(action));
        parts.push(
            div()
                .flex()
                .mt(u(10.0))
                .child(
                    Button::new("reset-keybinds")
                        .label(text::RESET_ALL_KEYBINDS)
                        .selector("reset-keybinds")
                        .disabled(!changed)
                        .h(u(28.0))
                        .px(u(10.0))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.keys.recording = None;
                            this.finish_keybind_recording(cx);
                            this.state.update(cx, |state, cx| {
                                let _ = state.change_keybinds(KeybindChange::ResetAll, cx);
                            });
                        })),
                )
                .into_any_element(),
        );
        if let Some(error) = self.state.read(cx).keybinds_error() {
            parts.push(super::settings::help_owned(error.to_owned(), theme).into_any_element());
        }
        for group in [
            ActionGroup::Call,
            ActionGroup::Navigation,
            ActionGroup::Messages,
            ActionGroup::App,
        ] {
            parts.push(
                super::settings::group_label(text::keybind_group(group), theme).into_any_element(),
            );
            for action in KeyAction::ALL
                .into_iter()
                .filter(|action| action.group() == group)
            {
                parts.push(self.render_keybind_row(action, theme, cx));
            }
        }
        parts
    }

    fn render_keybind_row(
        &mut self,
        action: KeyAction,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = theme.colors;
        let (keys, binding, changed, conflicts) = {
            let state = self.state.read(cx);
            let keybinds = state.keybinds();
            (
                state.keys_of(action),
                keybinds.get(action),
                keybinds.is_changed(action),
                keybinds.conflicts(action),
            )
        };
        let recording = self.keys.recording == Some(action);
        let shown = match (&keys, recording) {
            (_, true) => text::PRESS_KEYS.to_owned(),
            (Some(keys), false) => text::key_combo(keys),
            (None, false) => text::NO_KEYS.to_owned(),
        };
        let mut line = div()
            .flex()
            .items_center()
            .gap(u(8.0))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .typeface(theme.types.control, theme)
                    .ink(colors.ink)
                    .child(text::keybind_action(action)),
            )
            .child(
                div()
                    .flex_none()
                    .typeface(theme.types.mono_small, theme)
                    .ink(if recording {
                        colors.terracotta_deep
                    } else {
                        colors.ink
                    })
                    .debug_selector(move || format!("keybind-keys-{}", action.name()))
                    .child(shown),
            );
        if !recording {
            line = line.child(
                Button::new(selector("set", action))
                    .label(if keys.is_some() {
                        text::CHANGE_KEYS
                    } else {
                        text::SET_KEYS
                    })
                    .kind(ButtonKind::Ghost)
                    .selector(selector("set", action))
                    .h(u(26.0))
                    .px(u(8.0))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.keys.recording = Some(action);
                        this.keys.recording_release = true;
                        this.keys.talks = false;
                        this.keys.mutes = false;
                        let id = cx.entity_id();
                        this.state
                            .update(cx, |state, _| state.begin_keybind_recording(id));
                        this.keys.refusal = None;
                        cx.notify();
                    })),
            );
            if keys.is_some() {
                line = line.child(
                    Button::new(selector("remove", action))
                        .label(text::REMOVE_KEYS)
                        .kind(ButtonKind::Ghost)
                        .selector(selector("remove", action))
                        .h(u(26.0))
                        .px(u(8.0))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.state.update(cx, |state, cx| {
                                let _ = state.change_keybinds(KeybindChange::Set(action, None), cx);
                            });
                        })),
                );
            }
            if changed {
                line = line.child(
                    Button::new(selector("reset", action))
                        .label(text::RESET_KEYS)
                        .kind(ButtonKind::Ghost)
                        .selector(selector("reset", action))
                        .h(u(26.0))
                        .px(u(8.0))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.state.update(cx, |state, cx| {
                                let _ = state.change_keybinds(KeybindChange::Reset(action), cx);
                            });
                        })),
                );
            }
        }
        let mut row = div()
            .flex()
            .flex_col()
            .gap(u(2.0))
            .py(u(4.0))
            .debug_selector(move || format!("keybind-{}", action.name()))
            .child(line);
        if action.may_work_outside() {
            let outside = binding.outside;
            row = row.child(
                div()
                    .flex()
                    .items_center()
                    .gap(u(8.0))
                    .child(
                        div()
                            .flex_1()
                            .typeface(theme.types.small, theme)
                            .ink(colors.ink_2)
                            .child(text::KEYBIND_OUTSIDE),
                    )
                    .child(
                        Toggle::new(
                            selector("outside", action),
                            text::KEYBIND_OUTSIDE,
                            outside,
                            (text::ON, text::OFF),
                        )
                        .selector(selector("outside", action))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.state.update(cx, |state, cx| {
                                let _ = state
                                    .change_keybinds(KeybindChange::Outside(action, !outside), cx);
                            });
                        })),
                    ),
            );
        }
        if !conflicts.is_empty() {
            row = row.child(
                div()
                    .typeface(theme.types.small, theme)
                    .ink(colors.cinnabar_ink)
                    .debug_selector(move || format!("keybind-conflict-{}", action.name()))
                    .child(text::keybind_conflict(&conflicts)),
            );
        }
        if recording && let Some(refusal) = &self.keys.refusal {
            row = row.child(
                div()
                    .typeface(theme.types.small, theme)
                    .ink(colors.cinnabar_ink)
                    .debug_selector(|| "keybind-refusal".to_owned())
                    .child(refusal.clone()),
            );
        }
        if action == KeyAction::PushToTalk && !changed {
            row = row.child(super::settings::help(text::TALK_KEY_FROM_VOICE, theme));
        }
        row.into_any_element()
    }

    /// How the keys reach Junto on this system now, with the macOS permission and its
    /// buttons (ADR 0776).
    fn render_keys_reach(&mut self, theme: &Theme, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let state = self.state.read(cx).global_keys_state();
        let permission = junto_client_core::global_keys::input_monitoring();
        let (reach, asks) = keys_reach(state, permission, cfg!(target_os = "macos"));
        let mut rows = vec![
            div()
                .debug_selector(|| "keys-reach".to_owned())
                .child(super::settings::help_owned(reach, theme))
                .into_any_element(),
        ];
        if asks && cfg!(target_os = "macos") {
            rows.push(
                div()
                    .flex()
                    .gap(u(8.0))
                    .mt(u(6.0))
                    .child(
                        Button::new("allow-input-monitoring")
                            .label(text::ALLOW_INPUT_MONITORING)
                            .selector("allow-input-monitoring")
                            .h(u(28.0))
                            .px(u(10.0))
                            .on_click(cx.listener(|_, _, _, cx| {
                                junto_client_core::global_keys::request_input_monitoring();
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("open-input-monitoring")
                            .label(text::OPEN_INPUT_MONITORING)
                            .selector("open-input-monitoring")
                            .h(u(28.0))
                            .px(u(10.0))
                            .on_click(cx.listener(|_, _, _, cx| {
                                cx.open_url(
                                    junto_client_core::global_keys::INPUT_MONITORING_SETTINGS,
                                );
                            })),
                    )
                    .into_any_element(),
            );
        }
        rows
    }
}

/// The sentence of how the keys reach Junto, and whether the settings show the buttons of the
/// macOS permission. On macOS the permission decides first: before the first reading of a call,
/// or after a call ended, the source has no failure to tell.
fn keys_reach(state: KeysState, permission: Permission, macos: bool) -> (String, bool) {
    if macos && permission == Permission::NotGranted {
        return (text::KEYS_NEED_PERMISSION.to_owned(), true);
    }
    match state {
        KeysState::Reading if macos => (text::KEYS_ALLOWED.to_owned(), false),
        KeysState::Reading => (text::KEYS_EVERYWHERE.to_owned(), false),
        KeysState::Failed(ReadError::NotAllowed) => (text::KEYS_NEED_PERMISSION.to_owned(), macos),
        KeysState::Failed(ReadError::Failed(reason)) => (text::keys_portal_failed(&reason), false),
        KeysState::Portal => (text::KEYS_PORTAL.to_owned(), false),
        KeysState::PortalFailed(reason) => (text::keys_portal_failed(&reason), false),
        KeysState::WindowOnly => (text::KEYS_WINDOW_ONLY.to_owned(), false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_macos_permission_decides_before_the_last_reading() {
        let need = (text::KEYS_NEED_PERMISSION.to_owned(), true);
        for state in [
            KeysState::Reading,
            KeysState::Failed(ReadError::NotAllowed),
            KeysState::WindowOnly,
        ] {
            assert_eq!(
                keys_reach(state.clone(), Permission::NotGranted, true),
                need,
                "{state:?}"
            );
        }
        assert_eq!(
            keys_reach(KeysState::Reading, Permission::Granted, true),
            (text::KEYS_ALLOWED.to_owned(), false)
        );
        assert_eq!(
            keys_reach(KeysState::Reading, Permission::Granted, false),
            (text::KEYS_EVERYWHERE.to_owned(), false)
        );
        assert_eq!(
            keys_reach(KeysState::WindowOnly, Permission::Granted, true),
            (text::KEYS_WINDOW_ONLY.to_owned(), false)
        );
    }
}
