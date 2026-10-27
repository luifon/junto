//! The main area: the header of the selected channel, the statement of what this version
//! shows for it, the state of a server that has no lists, the recovery notices, and the
//! side panel with the members of the workspace.

use gpui::{
    AnyElement, BoxShadow, Context, Hsla, InteractiveElement, IntoElement, ParentElement, Role,
    SharedString, StatefulInteractiveElement, Styled, StyledText, Window, deferred, div,
    prelude::FluentBuilder, px,
};
use junto_client_core::protocol::WorkspacePermission;
use junto_client_core::protocol::ids::AccountId;
use junto_client_core::protocol::structure::ChannelKind;

use super::search::SEARCH_FIELD_WIDTH;
use super::{Layer, Shell};
use crate::assets::Icon;
use junto_media::calls::Phase;

use crate::state::view::{
    CallPanel, ChannelRow, Members, PanelPlace, RecordingControl, RecordingShown, Relation,
    SelectedServer, ServerLists, ServerStatus, ShellView, VoiceState,
};
use crate::text;
use crate::theme::{Theme, theme, u};
use crate::ui::{
    Button, ButtonKind, Flag, FlagKind, Notice, ScrollArea, SectionLabel, SideHeader, StatePanel,
    StyleExt, float_shadow, icon, tooltip,
};

/// The time of a recovery as the notice shows it: the date and the time in UTC.
fn recovery_time(notice: &junto_client_core::protocol::account::RecoveryNotice) -> String {
    let at = crate::moment::in_utc(notice.performed_at);
    format!(
        "{} {} {} at {:02}:{:02} UTC",
        at.day(),
        at.month(),
        at.year(),
        at.hour(),
        at.minute()
    )
}

impl Shell {
    pub(crate) fn render_main(
        &mut self,
        view: &ShellView,
        narrow: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = theme(cx);
        let lists = view.lists();
        let workspace = view.workspace();
        let channel = workspace.and_then(|workspace| workspace.channel.as_ref());
        // The call of this device, when its voice channel or the direct conversation of its
        // direct-message call is the shown place.
        let call = lists.and_then(|lists| lists.call.as_ref());
        let members_open = self.members_open && lists.is_some() && call.is_none();
        let server_id = view.selected.as_ref().map(|server| server.id);
        // The side panel shows one thing: a thread, the pinned messages, the notices of
        // the server, a search, or the members. Without a workspace only the notices.
        let side_of =
            |this: &mut Self, lists: &ServerLists, window: &mut Window, cx: &mut Context<Self>| {
                let shown = lists.workspace.as_ref();
                if let (Some(shown), Some(server_id)) = (shown, server_id)
                    && shown.thread.is_none()
                    && shown.pins.is_none()
                    && let Some(transcript) = &shown.transcript
                {
                    return Some(this.render_transcript_panel(server_id, transcript, window, cx));
                }
                let thread = shown.and_then(|shown| shown.thread.as_ref());
                let pins = shown.and_then(|shown| shown.pins.as_ref());
                let search = shown.and_then(|shown| shown.search.as_ref());
                match (thread, pins, &lists.notices, search) {
                    (Some(thread), ..) => Some(this.render_thread_panel(thread, window, cx)),
                    (None, Some(pins), ..) => {
                        Some(this.render_pins_panel(pins, lists.feed.as_ref(), window, cx))
                    }
                    (None, None, Some(notices), _) => server_id
                        .map(|server_id| this.render_notices_panel(server_id, notices, false, cx)),
                    (None, None, None, Some(search)) => {
                        Some(this.render_search_panel(search, window, cx))
                    }
                    _ => members_open.then(|| this.render_members(lists, &theme, cx)),
                }
            };

        // State panels keep their notices below the actions (ADR 0730).
        let mut notices = Some(self.render_notices(view, &theme, cx));
        let stated = |panel: StatePanel, notices: &mut Option<Vec<AnyElement>>| {
            panel
                .after_actions(notices.take().unwrap_or_default())
                .into_any_element()
        };
        let (body, side): (AnyElement, Option<AnyElement>) = match (&view.selected, lists) {
            (None, _) => (
                stated(
                    StatePanel::new(text::NO_SERVER_TITLE, text::NO_SERVER).selector("no-server"),
                    &mut notices,
                ),
                None,
            ),
            (Some(server), Some(lists)) => match (call, channel, lists.conversation.as_ref()) {
                (Some(panel), _, _) => self.render_call(view, panel, narrow, window, cx),
                (None, Some(_), _) => (
                    self.render_feed(lists, false, notices.take().unwrap_or_default(), window, cx),
                    side_of(self, lists, window, cx),
                ),
                // A direct conversation in place of a channel, also on a server where the
                // account can see no workspace (ADR 0741).
                (None, None, Some(conversation)) => (
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_h_0()
                        .child(self.render_direct_call(server.id, conversation.account_id, cx))
                        .child(self.render_feed(
                            lists,
                            false,
                            notices.take().unwrap_or_default(),
                            window,
                            cx,
                        ))
                        .into_any_element(),
                    side_of(self, lists, window, cx),
                ),
                (None, None, None) => (
                    stated(
                        match lists.workspace {
                            Some(_) => StatePanel::new(text::NO_CHANNEL_TITLE, text::NO_CHANNELS)
                                .selector("no-channel"),
                            None => StatePanel::new(text::NO_CHANNEL_TITLE, text::NO_WORKSPACES)
                                .selector("no-workspace"),
                        },
                        &mut notices,
                    ),
                    side_of(self, lists, window, cx),
                ),
            },
            // A server that waits for a device link shows its code and the key of this
            // device (ADR 0782).
            (Some(server), None) => (
                stated(
                    match self.render_link_wait_panel(server, cx) {
                        Some(panel) => panel,
                        None => self.render_server_state(server, cx),
                    },
                    &mut notices,
                ),
                None,
            ),
        };

        let in_call = call.is_some();
        let thread_open = workspace.is_some_and(|workspace| workspace.thread.is_some());
        let pins_open = workspace.is_some_and(|workspace| workspace.pins.is_some());
        let notices_open = lists.is_some_and(|lists| lists.notices.is_some());
        let transcript_open = workspace.is_some_and(|workspace| workspace.transcript.is_some());
        let search_open = workspace.is_some_and(|workspace| workspace.search.is_some());
        let split = div()
            .relative()
            .flex()
            .flex_1()
            .min_h_0()
            .child(if in_call {
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .child(body)
                    .child(
                        crate::ui::notice_slot("call-notice-area")
                            .h(u(128.0))
                            .children(
                                call.and_then(|panel| panel.room())
                                    .and_then(|room| room.meeting.as_ref())
                                    .map(|meeting| self.render_meeting_bar(meeting, &theme, cx)),
                            )
                            .children(notices.take().into_iter().flatten()),
                    )
            } else {
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .children(notices.take().into_iter().flatten())
                    .child(body)
            })
            .when_some(side, |split, side| {
                if narrow {
                    // The panel lies over the content, with a flat cover behind it.
                    split
                        .child(Self::side_cover(&theme, cx, move |this, cx| {
                            if in_call {
                                this.call_panel_open = false;
                            } else if thread_open {
                                this.state.update(cx, |state, cx| state.close_thread(cx));
                            } else if pins_open {
                                this.state.update(cx, |state, cx| state.close_pins(cx));
                            } else if transcript_open {
                                this.state
                                    .update(cx, |state, cx| state.close_transcript(cx));
                            } else if notices_open {
                                this.state.update(cx, |state, cx| state.close_notices(cx));
                            } else if search_open {
                                this.state.update(cx, |state, cx| state.hide_search(cx));
                            } else {
                                this.members_open = false;
                            }
                        }))
                        .child(Self::side_overlay(&theme, side))
                } else {
                    split.child(side)
                }
            });

        div()
            .id("main")
            .role(Role::Main)
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(self.render_main_header(view, channel, call, narrow, &theme, cx))
            .child(split)
    }

    fn render_main_header(
        &mut self,
        view: &ShellView,
        channel: Option<&ChannelRow>,
        call: Option<&CallPanel>,
        narrow: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = theme.colors;
        let members_open = self.members_open && call.is_none();
        let conversation = view.lists().and_then(|lists| lists.conversation.as_ref());
        let title: Option<(Icon, SharedString)> = match (conversation, channel) {
            (Some(conversation), _) => Some((
                Icon::Person,
                conversation
                    .name
                    .clone()
                    .unwrap_or_else(|| text::UNKNOWN_PERSON.to_owned())
                    .into(),
            )),
            (None, Some(channel)) => Some((
                match channel.kind {
                    ChannelKind::Text => Icon::Hash,
                    ChannelKind::Voice => Icon::Speaker,
                },
                channel.name.clone().into(),
            )),
            (None, None) => None,
        };
        let server_id = view.selected.as_ref().map(|server| server.id);
        let pins_open = view
            .workspace()
            .is_some_and(|workspace| workspace.pins.is_some());
        let unread = match (conversation, channel) {
            (Some(conversation), _) => conversation.unread.unread(),
            (None, Some(channel)) => channel.unread.unread(),
            (None, None) => false,
        };
        let permanent = channel.is_some_and(|channel| channel.permanent) && call.is_none();
        let voice = channel.and_then(|channel| channel.voice.as_ref());
        // What follows the title: for a voice channel that this device is not in, the
        // line about its chat and the action that enters the call; for the call of this
        // device, its state.
        let mut after: Vec<AnyElement> = Vec::new();
        let outside = voice.filter(|voice| voice.relation != Relation::Inside);
        if let (Some(channel), Some(voice)) = (channel, outside) {
            // The divider and the line about the chat, the states of the call, and the
            // action that enters it.
            let mut entry = self
                .render_voice_entry(view, channel, voice, theme, cx)
                .into_iter();
            after.extend(entry.by_ref().take(2));
            after.extend(voice_header_states(voice, theme));
            after.extend(entry);
        } else if let Some(panel) = call {
            if matches!(panel.phase, Phase::Connecting | Phase::Entering) {
                after.push(div().child(text::CALL_ENTERING_HELP).into_any_element());
                after.push(div().flex_1().into_any_element());
            } else if panel.phase == Phase::Reconnecting {
                after.extend(Self::render_call_interruption(theme));
                after.push(div().flex_1().into_any_element());
            } else {
                after.push(
                    div()
                        .flex_none()
                        .w(px(1.0))
                        .h(u(20.0))
                        .bg(Hsla::from(colors.rule))
                        .into_any_element(),
                );
                match &panel.place {
                    // The recording of the call, and then the state of the room.
                    PanelPlace::Channel(channel) => {
                        let recording = &channel.recording;
                        after.extend(self.render_recording_control(recording, theme, false, cx));
                        after.push(div().flex_1().into_any_element());
                        after.extend(self.render_room_controls(&channel.room, theme, cx));
                    }
                    // Whether the other person has taken a place in the call.
                    PanelPlace::Direct(direct) => {
                        let (label, selector) = if direct.joined {
                            (text::DIRECT_CALL, "active-direct-call")
                        } else {
                            (text::CALLING, "outgoing-direct-call")
                        };
                        after.push(
                            div()
                                .id(selector)
                                .role(Role::Status)
                                .aria_label(label)
                                .debug_selector(move || selector.to_owned())
                                .flex_none()
                                .map(|state| {
                                    if direct.joined {
                                        state.child(crate::ui::Chip::new(label).icon(Icon::Phone))
                                    } else {
                                        state.child(
                                            Flag::new(FlagKind::Warning, label)
                                                .icon(Icon::PhoneOutgoing),
                                        )
                                    }
                                })
                                .into_any_element(),
                        );
                        after.push(div().flex_1().into_any_element());
                    }
                }
            }
            if narrow {
                after.extend(self.render_call_panel_buttons(cx));
            }
        } else {
            if permanent {
                after.push(Self::render_permanent_marker(theme));
            }
            if self.shown_archived().is_some() {
                after.push(
                    div()
                        .flex_none()
                        .debug_selector(|| "archived-mark".to_owned())
                        .child(crate::ui::Chip::new(text::admin::ARCHIVED_MARK).icon(Icon::Archive))
                        .into_any_element(),
                );
            }
            after.push(div().flex_1().into_any_element());
        }
        super::title_bar::header_zone(div())
            .id("main-header")
            .relative()
            .flex()
            .flex_none()
            .items_center()
            // The header of a call in a narrow window holds the controls of the room and
            // the buttons of the panel: its parts stand closer.
            .gap(u(if narrow && call.is_some() { 6.0 } else { 10.0 }))
            .h(u(theme.metrics.header_height))
            .pl(u(16.0))
            .pr(u(if narrow && call.is_some() { 8.0 } else { 14.0 }))
            .bg(Hsla::from(colors.travertine))
            .border_b_1()
            .border_color(Hsla::from(colors.rule))
            // The double line under the header.
            .shadow(vec![
                BoxShadow::new(px(0.0), px(3.0), colors.rule_soft.into()),
                BoxShadow::new(px(0.0), px(2.0), colors.travertine.into()),
            ])
            .when_some(title, |header, (shown, name)| {
                header.child(
                    div()
                        .id("channel-title")
                        .role(Role::Heading)
                        .aria_level(1)
                        .aria_label(name.clone())
                        .debug_selector(|| "channel-title".to_owned())
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(u(7.0))
                        .max_w(u(420.0))
                        .typeface(theme.types.title, theme)
                        .ink(colors.ink)
                        .child(icon(shown, 17.0, colors.terracotta_deep))
                        .child(div().min_w_0().truncate().child(name)),
                )
            })
            .children(after)
            .when(call.is_none() && unread, |header| {
                header.child(
                    Button::new("mark-as-read")
                        .icon(Icon::CheckDouble)
                        .label(text::MARK_AS_READ)
                        .tooltip(text::MARK_AS_READ_HELP)
                        .selector("mark-as-read")
                        .on_click(cx.listener(|this, _, _, cx| this.mark_shown_as_read(cx))),
                )
            })
            .when_some(
                server_id
                    .zip(channel.map(|channel| channel.id))
                    .filter(|_| call.is_none()),
                |header, (server_id, channel_id)| {
                    // The pinned messages are read from the history (ADR 0790).
                    let refused = channel
                        .filter(|channel| !channel.allowed.read_history)
                        .map(|_| text::not_permitted(WorkspacePermission::ReadMessageHistory));
                    header.child(
                        Button::new("pins")
                            .icon(Icon::Pin)
                            .label(text::PINS)
                            .tooltip(match refused {
                                Some(reason) => reason,
                                None if pins_open => text::CLOSE_PINS,
                                None => text::SHOW_PINS,
                            })
                            .accessible_label(text::PINS)
                            .disabled(refused.is_some())
                            .selector(if refused.is_some() {
                                "pins-button-refused"
                            } else {
                                "pins-button"
                            })
                            .when(pins_open, |button| {
                                button.bg(Hsla::from(colors.limestone_lo)).shadow_none()
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if pins_open {
                                    this.state.update(cx, |state, cx| state.close_pins(cx));
                                } else {
                                    this.open_pins(server_id, channel_id, cx);
                                }
                            })),
                    )
                },
            )
            .when(
                channel.is_some() && conversation.is_none() && call.is_none(),
                |header| {
                    header.child(
                        Button::new("channel-settings")
                            .icon(Icon::Gear)
                            .tooltip(text::admin::CHANNEL_SETTINGS)
                            .accessible_label(text::admin::CHANNEL_SETTINGS)
                            .selector("channel-settings")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_shown_channel_settings(window, cx);
                            })),
                    )
                },
            )
            .when(conversation.is_some() && call.is_none(), |header| {
                header.child(
                    Button::new("export-conversation")
                        .icon(Icon::Download)
                        .tooltip(text::admin::EXPORT_CONVERSATION)
                        .accessible_label(text::admin::EXPORT_CONVERSATION)
                        .selector("export-conversation")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.export_shown_conversation(window, cx);
                        })),
                )
            })
            .when(view.lists().is_some() && call.is_none(), |header| {
                header.child(
                    Button::new("members")
                        .icon(Icon::People)
                        .label(text::MEMBERS)
                        .tooltip(if members_open {
                            text::CLOSE_MEMBERS
                        } else {
                            text::SHOW_MEMBERS
                        })
                        .accessible_label(text::MEMBERS)
                        .selector("members-button")
                        .when(members_open, |button| {
                            button.bg(Hsla::from(colors.limestone_lo)).shadow_none()
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.members_open = !this.members_open;
                            if this.members_open {
                                // The side panel shows one thing.
                                this.state.update(cx, |state, cx| {
                                    state.close_thread(cx);
                                    state.close_pins(cx);
                                    state.close_notices(cx);
                                    state.hide_search(cx);
                                    state.close_transcript(cx);
                                });
                            }
                            cx.notify();
                        })),
                )
            })
            // Without a workspace there is no search (ADR 0743): its place stays empty, so
            // that the controls before it do not move when a workspace goes or comes
            // (ADR 0730).
            .when(view.lists().is_some() && call.is_none(), |header| {
                header.child(match view.workspace() {
                    Some(_) => self.render_search_field(cx),
                    None => div()
                        .flex_none()
                        .w(u(SEARCH_FIELD_WIDTH))
                        .debug_selector(|| "search-place".to_owned())
                        .into_any_element(),
                })
            })
    }

    /// The marker of a channel whose messages are permanent, with its explanation on
    /// hover and on focus (view J of the design, ADR 0402).
    fn render_permanent_marker(theme: &Theme) -> AnyElement {
        let colors = theme.colors;
        let ring = crate::ui::focus_ring_inside(colors.terracotta_deep, theme.metrics.focus_ring);
        let about: SharedString = format!("{}{}", text::PERMANENT_ON, text::PERMANENT_ABOUT).into();
        div()
            .id("permanent-marker")
            .role(Role::Note)
            .aria_label(about.clone())
            .debug_selector(|| "permanent-marker".to_owned())
            .flex()
            .flex_none()
            .items_center()
            .gap(u(6.0))
            .h(u(26.0))
            .px(u(8.0))
            .rounded(u(theme.metrics.radius))
            .border_1()
            .border_color(Hsla::from(colors.rule))
            .bg(Hsla::from(colors.plaster))
            .typeface(theme.types.control_strong, theme)
            .text_size(u(13.0))
            .ink(colors.ink)
            .whitespace_nowrap()
            .focusable()
            .tab_stop(true)
            .focus_visible(move |style| style.shadow(ring))
            .tooltip(tooltip(about))
            .child(icon(Icon::EditOff, 14.0, colors.terracotta_deep))
            .child(text::PERMANENT_MESSAGES)
            .into_any_element()
    }

    /// The recording of the call of this device in the header of its voice channel
    /// (ADR 0738): the mark with its time and the action that stops it, the action that
    /// starts one, and what happened to the last one. The `compact` form of the call
    /// window draws each action as an icon with its tooltip, and shortens the words of the
    /// mark, the flag of a failure and the sentence of a missing permission with an
    /// ellipsis where the place is too narrow (ADR 0772).
    pub(crate) fn render_recording_control(
        &mut self,
        control: &RecordingControl,
        theme: &Theme,
        compact: bool,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let colors = theme.colors;
        let labelled = |button: Button, label: &'static str| {
            if compact { button } else { button.label(label) }
        };
        // "Record" opens the languages of the speech; a choice starts the recording
        // (ADR 0616).
        let offers_record = control.may_record
            && matches!(
                control.state,
                RecordingShown::None | RecordingShown::Failed(_)
            );
        if matches!(self.layer, Layer::RecordLanguage) && (!offers_record || control.busy) {
            self.layer = Layer::None;
        }
        let languages = (matches!(self.layer, Layer::RecordLanguage) && offers_record)
            .then(|| self.render_record_languages(cx));
        let record = |title: &'static str, cx: &mut Context<Self>| {
            div()
                .relative()
                .flex()
                .flex_none()
                .child(
                    labelled(Button::new("start-recording"), text::RECORD)
                        .icon(Icon::Record)
                        .icon_colour(colors.cinnabar)
                        .tooltip(title)
                        .accessible_label(title)
                        .disabled(control.busy)
                        .selector("start-recording")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_record_languages(window, cx);
                        })),
                )
                .children(languages)
                .into_any_element()
        };
        let why = |sentence: &'static str| {
            div()
                .id("no-record-permission")
                .debug_selector(|| "no-record-permission".to_owned())
                .min_w_0()
                .truncate()
                .typeface(theme.types.small, theme)
                .text_size(u(13.0))
                .ink(if self.detached {
                    colors.bone_2
                } else {
                    colors.ink_2
                })
                .child(sentence)
                .into_any_element()
        };
        let forbidden = || {
            vec![
                labelled(Button::new("start-recording"), text::RECORD)
                    .icon(Icon::Record)
                    .tooltip(text::NO_RECORD_PERMISSION)
                    .disabled(true)
                    .selector("start-recording")
                    .into_any_element(),
                why(text::NO_RECORD_PERMISSION),
            ]
        };
        match &control.state {
            RecordingShown::None if control.may_record => vec![record(text::START_RECORDING, cx)],
            RecordingShown::None => forbidden(),
            // The mark keeps one width while the recording starts and runs, so the button
            // after it stays in place when "Cancel" becomes "Stop" (ADR 0730).
            RecordingShown::Starting => {
                let mut shown = vec![recording_mark_reserved(
                    format!("{} {}", text::RECORDING_MARK, text::RECORDING_STARTS).into(),
                    compact,
                    theme,
                )];
                if control.may_record {
                    shown.push(
                        labelled(
                            Button::new("cancel-recording"),
                            text::transcripts::CANCEL_RECORDING,
                        )
                        .icon(Icon::Stop)
                        .icon_colour(colors.cinnabar)
                        .tooltip(text::transcripts::CANCEL_RECORDING_HELP)
                        .accessible_label(text::transcripts::CANCEL_RECORDING_HELP)
                        .disabled(control.busy)
                        .selector("cancel-recording")
                        .on_click(self.call_listener(cx, |this, _, _, cx| {
                            this.state.update(cx, |state, cx| state.stop_recording(cx));
                        }))
                        .into_any_element(),
                    );
                }
                shown
            }
            RecordingShown::Running { elapsed, stopping } => {
                let mark: SharedString = match elapsed {
                    Some(elapsed) => text::recording_time(*elapsed).into(),
                    None => text::RECORDING_MARK.into(),
                };
                let mut shown = vec![recording_mark_reserved(mark, compact, theme)];
                if control.may_record {
                    let (label, help) = stop_action(*stopping);
                    shown.push(
                        labelled(Button::new("stop-recording"), label)
                            .icon(Icon::Stop)
                            .icon_colour(colors.cinnabar)
                            .tooltip(help)
                            .accessible_label(help)
                            .disabled(*stopping || control.busy)
                            .selector("stop-recording")
                            .on_click(self.call_listener(cx, |this, _, _, cx| {
                                this.state.update(cx, |state, cx| state.stop_recording(cx));
                            }))
                            .into_any_element(),
                    );
                }
                shown
            }
            RecordingShown::Failed(problem) => {
                let (strong, rest) = text::recording_details(problem);
                let open = self.recording_details_open;
                let details = div()
                    .relative()
                    .flex()
                    .flex_none()
                    .child(
                        Button::new("recording-details")
                            .label(text::DETAILS)
                            .tooltip(text::RECORDING_DETAILS)
                            .selector("recording-details")
                            .on_click(cx.listener(|this, _, _, cx| {
                                // A click on the button of open details closes them: the
                                // press closed them, and the click does not open them.
                                if std::mem::take(&mut this.recording_details_closed_by_press) {
                                    return;
                                }
                                this.recording_details_open = !this.recording_details_open;
                                cx.notify();
                            })),
                    )
                    .when(open, |details| {
                        let line = format!("{strong}{rest}");
                        details.child(
                            div().absolute().left_0().top(u(32.0)).child(deferred(
                                div()
                                    .id("recording-details-text")
                                    .role(Role::Status)
                                    .aria_label(line.clone())
                                    .debug_selector(|| "recording-details-text".to_owned())
                                    .occlude()
                                    .w(u(340.0))
                                    .pt(u(8.0))
                                    .px(u(10.0))
                                    .pb(u(9.0))
                                    .rounded(u(theme.metrics.radius))
                                    .border_1()
                                    .border_color(Hsla::from(colors.rule))
                                    .bg(Hsla::from(colors.travertine_hi))
                                    .shadow(float_shadow(colors.shadow, 6.0, 16.0))
                                    .typeface(theme.types.small, theme)
                                    .line_height(u(17.0))
                                    .font_weight(gpui::FontWeight::NORMAL)
                                    .ink(colors.ink_2)
                                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                        this.recording_details_open = false;
                                        this.recording_details_closed_by_press = true;
                                        cx.notify();
                                    }))
                                    .child(StyledText::new(line).with_highlights([(
                                        0..strong.len(),
                                        gpui::HighlightStyle {
                                            font_weight: Some(gpui::FontWeight::BOLD),
                                            color: Some(Hsla::from(colors.ink)),
                                            ..gpui::HighlightStyle::default()
                                        },
                                    )])),
                            )),
                        )
                    })
                    .into_any_element();
                let mut shown = vec![
                    div()
                        .id("recording-failed")
                        .role(Role::Status)
                        .aria_label(text::recording_flag(problem))
                        .debug_selector(|| "recording-failed".to_owned())
                        .map(|failed| {
                            if compact {
                                failed.flex().min_w_0()
                            } else {
                                failed.flex_none()
                            }
                        })
                        .child(
                            Flag::new(FlagKind::Error, text::recording_flag(problem))
                                .icon(Icon::Warning)
                                .when(compact, Flag::shrinks),
                        )
                        .into_any_element(),
                    details,
                ];
                if control.may_record {
                    shown.push(record(text::START_NEW_RECORDING, cx));
                }
                shown
            }
        }
    }

    /// The notices about the one call of this client: the rules of the call, how the last
    /// join or call ended, a share that ended by itself, and a request for a recording
    /// that the server did not take. The main window and the call window draw them.
    pub(crate) fn render_call_notices(
        &mut self,
        view: &ShellView,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        // How the last join or call ended. The notice concerns the one call of this
        // client, so it is shown whatever server is selected.
        let call_notice = view.call_notice.as_ref().map(|notice| {
            div()
                .flex_none()
                .px(u(16.0))
                .pt(u(10.0))
                .child(
                    Notice::new(text::call_notice(notice))
                        .icon(Icon::Warning)
                        .selector("call-notice")
                        .action(
                            Button::new("dismiss-call-notice")
                                .label(text::DISMISS)
                                .selector("dismiss-call-notice")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.state
                                        .update(cx, |state, _| state.dismiss_call_notice());
                                })),
                        ),
                )
                .into_any_element()
        });
        // The share of this device that ended by itself, and a request for a recording that
        // the server did not take. Both concern the one call of this client.
        let share_notice = view.share_notice.as_ref().map(|error| {
            div()
                .flex_none()
                .px(u(16.0))
                .pt(u(10.0))
                .child(
                    Notice::new(text::share_ended(error))
                        .icon(Icon::Warning)
                        .selector("share-notice")
                        .action(
                            Button::new("dismiss-share-notice")
                                .label(text::DISMISS)
                                .selector("dismiss-share-notice")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.state
                                        .update(cx, |state, cx| state.dismiss_share_notice(cx));
                                })),
                        ),
                )
                .into_any_element()
        });
        let pending = self.state.read(cx).pending().clone();
        let recording_notice = pending.recording_failed.as_ref().map(|reason| {
            div()
                .flex_none()
                .px(u(16.0))
                .pt(u(10.0))
                .child(
                    Notice::error(text::recording_request_failed(reason))
                        .selector("recording-error")
                        .action(
                            Button::new("dismiss-recording-error")
                                .label(text::DISMISS)
                                .selector("dismiss-recording-error")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.state
                                        .update(cx, |state, cx| state.dismiss_recording_error(cx));
                                })),
                        ),
                )
                .into_any_element()
        });
        let rules = self.render_call_rule_notices(view, cx);
        let about_call = || {
            rules
                .into_iter()
                .chain(call_notice)
                .chain(share_notice)
                .chain(recording_notice)
        };
        about_call().collect()
    }

    /// The notices above the content: how the last call ended, and the recoveries that
    /// this device has not acknowledged (ADR 0118). A recovery notice goes only when the
    /// person presses its button.
    fn render_notices(
        &mut self,
        view: &ShellView,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut notices = self.render_call_notices(view, cx);
        let pending = self.state.read(cx).pending().clone();
        let Some(server) = &view.selected else {
            return notices;
        };
        let server_id = server.id;
        let owner = self.state.read(cx).owner(server_id);
        notices.extend(
            server
                .recoveries
                .iter()
                .enumerate()
                .map(|(index, recovery)| {
                    let recovery_id = recovery.id;
                    let sending = pending.acknowledges(owner, recovery_id);
                    div()
                        .flex_none()
                        .px(u(16.0))
                        .pt(u(10.0))
                        .child(
                            Notice::new(text::recovery(
                                &server.name,
                                &recovery_time(recovery),
                                recovery.performed_by.is_none(),
                            ))
                            .icon(Icon::Warning)
                            .selector("recovery-notice")
                            .action(
                                Button::new(("recovery-seen", index))
                                    .kind(ButtonKind::Primary)
                                    .label(if sending {
                                        text::RECOVERY_SENDING
                                    } else {
                                        text::RECOVERY_SEEN
                                    })
                                    .disabled(sending)
                                    .selector("recovery-seen")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.state.update(cx, |state, cx| {
                                            state.acknowledge_recovery(server_id, recovery_id, cx);
                                        });
                                    })),
                            ),
                        )
                        .into_any_element()
                }),
        );
        // The private moderation notices of the account (ADR 0120).
        notices.extend(server.moderation.iter().map(|line| {
            let notice_id = line.id;
            let mut notice = Notice::new(text::moderation_notice(line))
                .icon(Icon::Warning)
                .selector("moderation-notice");
            if line.dismissable {
                notice = notice.action(
                    Button::new(("moderation-seen", notice_id.as_u128() as u64))
                        .label(text::DISMISS)
                        .selector("moderation-seen")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.state.update(cx, |state, cx| {
                                state.put_moderation_notice_away(server_id, notice_id, cx);
                            });
                        })),
                );
            }
            div()
                .flex_none()
                .px(u(16.0))
                .pt(u(10.0))
                .debug_selector(move || format!("moderation-notice:{notice_id}"))
                .child(notice)
                .into_any_element()
        }));
        if let Some(error) = &server.moderation_failed {
            notices.push(
                div()
                    .flex_none()
                    .px(u(16.0))
                    .pt(u(10.0))
                    .child(Notice::error(error.clone()).selector("moderation-away-failed"))
                    .into_any_element(),
            );
        }
        if let Some(error) = pending
            .remove_failed
            .get(&server_id)
            .filter(|_| view.workspace().is_some())
        {
            notices.push(
                div()
                    .flex_none()
                    .px(u(16.0))
                    .pt(u(10.0))
                    .child(Notice::error(error.clone()).selector("remove-error"))
                    .into_any_element(),
            );
        }
        if server.account_changed {
            notices.push(
                div()
                    .flex_none()
                    .px(u(16.0))
                    .pt(u(10.0))
                    .child(
                        Notice::new(text::ACCOUNT_CHANGED)
                            .icon(Icon::Warning)
                            .selector("account-changed"),
                    )
                    .into_any_element(),
            );
        }
        if let Some(error) = pending.acknowledge_failure(owner) {
            notices.push(
                div()
                    .flex_none()
                    .px(u(16.0))
                    .pt(u(10.0))
                    .child(Notice::error(error.to_owned()).selector("recovery-error"))
                    .into_any_element(),
            );
        }
        let _ = theme;
        // A direct conversation that the server did not open, unless the member list
        // beside the conversation states it at the member (ADR 0741). It comes after the
        // other notices, so that its arrival and its end move none of them (ADR 0730).
        if let Some(lists) = view.lists()
            && let Some(refusal) = &lists.conversation_refused
            && !self.refusal_at_member(view, refusal.account_id)
        {
            let account_id = refusal.account_id;
            let asked = lists.conversation_asked == Some(account_id);
            notices.push(
                div()
                    .flex_none()
                    .px(u(16.0))
                    .pt(u(10.0))
                    .child(
                        Notice::error(text::conversation_not_opened(
                            refusal.name.as_deref().unwrap_or(text::UNKNOWN_PERSON),
                            &refusal.reason,
                        ))
                        .selector("conversation-refused-notice")
                        .action(
                            div()
                                .flex()
                                .gap(u(6.0))
                                .child(
                                    Button::new("conversation-retry")
                                        .label(text::TRY_AGAIN)
                                        .icon(Icon::Retry)
                                        .disabled(asked)
                                        .selector("conversation-retry")
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.state.update(cx, |state, cx| {
                                                state.open_conversation(server_id, account_id, cx);
                                            });
                                        })),
                                )
                                .child(
                                    Button::new("conversation-refusal-close")
                                        .kind(ButtonKind::Ghost)
                                        .label(text::DISMISS)
                                        .disabled(asked)
                                        .selector("conversation-refusal-close")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.state.update(cx, |state, cx| {
                                                state.dismiss_conversation_refusal(cx);
                                            });
                                        })),
                                ),
                        ),
                    )
                    .into_any_element(),
            );
        }
        notices
    }

    /// The state of a server that shows no lists, with the actions that the state accepts.
    fn render_server_state(
        &mut self,
        server: &SelectedServer,
        cx: &mut Context<Self>,
    ) -> StatePanel {
        let server_id = server.id;
        let busy = server.busy || self.state.read(cx).cancels_request(server_id);
        let error: Option<SharedString> = server
            .error
            .as_ref()
            .map(|error| text::flow_error(error).into())
            .or_else(|| {
                self.state
                    .read(cx)
                    .pending()
                    .remove_failed
                    .get(&server_id)
                    .cloned()
                    .map(SharedString::from)
            });
        let check = |label: &'static str, cx: &mut Context<Self>| {
            Button::new("check-server")
                .label(label)
                .icon(Icon::Retry)
                .disabled(busy)
                .selector("check-server")
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.state
                        .update(cx, |state, cx| state.connect(server_id, cx));
                }))
        };
        let remove = |cx: &mut Context<Self>| {
            Button::new("remove-server")
                .label(text::REMOVE_SERVER)
                .icon(Icon::Trash)
                .selector("remove-server")
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_layer(Layer::ConfirmRemove(server_id), window, cx);
                }))
        };
        let forget = |cx: &mut Context<Self>| {
            Button::new("forget-account")
                .label(text::FORGET_ACCOUNT)
                .selector("forget-account")
                .disabled(busy)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_layer(Layer::ConfirmForget(server_id), window, cx);
                }))
        };
        let panel = match &server.status {
            ServerStatus::Ready
            | ServerStatus::Reconnecting
            | ServerStatus::Standby
            | ServerStatus::Connecting => {
                StatePanel::new(text::CONNECTING_TITLE, text::CONNECTING).selector("connecting")
            }
            ServerStatus::Unreachable => {
                StatePanel::new(text::UNREACHABLE_TITLE, text::UNREACHABLE)
                    .icon(Icon::Warning)
                    .selector("unreachable")
                    .action(check(text::TRY_AGAIN, cx))
                    .action(remove(cx))
            }
            ServerStatus::LoggedOut => StatePanel::new(text::LOGGED_OUT_TITLE, text::LOGGED_OUT)
                .icon(Icon::Warning)
                .selector("logged-out")
                .action(check(text::TRY_AGAIN, cx))
                .action(remove(cx)),
            ServerStatus::LoggedOutByPerson => {
                StatePanel::new(text::LOGGED_OUT_BY_PERSON_TITLE, text::LOGGED_OUT_BY_PERSON)
                    .selector("logged-out-by-person")
                    .action(
                        Button::new("log-in")
                            .kind(ButtonKind::Primary)
                            .label(text::LOG_IN)
                            .selector("log-in")
                            .disabled(busy)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.state
                                    .update(cx, |state, cx| state.log_in(server_id, cx));
                            })),
                    )
                    .action(remove(cx))
            }
            ServerStatus::WaitingForApproval { name } => StatePanel::new(
                text::REQUEST_SENT_TITLE,
                text::request_sent(&server.name, name),
            )
            .icon(Icon::Clock)
            .selector("waiting-for-approval")
            .action(check(text::CHECK_AGAIN, cx))
            .action(
                Button::new("cancel-request")
                    .label(text::CANCEL_REQUEST)
                    .selector("cancel-request")
                    .disabled(busy)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.state
                            .update(cx, |state, cx| state.cancel_request(server_id, cx));
                    })),
            ),
            ServerStatus::WaitingForLink { code } => StatePanel::new(
                text::WAITING_FOR_LINK_TITLE,
                format!("{} {}", text::WAITING_FOR_LINK, text::link_code(code)),
            )
            .icon(Icon::Clock)
            .selector("waiting-for-link")
            .action(check(text::CHECK_AGAIN, cx))
            .action(forget(cx)),
            ServerStatus::NoAccount(notice) => {
                let (title, sentence) = text::no_account(*notice);
                StatePanel::new(title, sentence)
                    .selector("no-account")
                    .action(
                        Button::new("join-server")
                            .kind(ButtonKind::Primary)
                            .label(text::JOIN_THIS_SERVER)
                            .selector("join-server")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_join_for(server_id, window, cx);
                            })),
                    )
                    .action(remove(cx))
            }
            ServerStatus::Banned => StatePanel::new(text::BANNED_TITLE, text::BANNED)
                .icon(Icon::Warning)
                .selector("banned")
                .action(check(text::CHECK_AGAIN, cx))
                .action(forget(cx))
                .action(remove(cx)),
            ServerStatus::Inactive => StatePanel::new(text::INACTIVE_TITLE, text::INACTIVE)
                .selector("inactive")
                .action(
                    Button::new("return-to-server")
                        .kind(ButtonKind::Primary)
                        .label(text::RETURN_WITH_CODE)
                        .selector("return-to-server")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_join_for(server_id, window, cx);
                        })),
                )
                .action(remove(cx)),
            ServerStatus::Deleted => StatePanel::new(text::DELETED_TITLE, text::DELETED)
                .icon(Icon::Warning)
                .selector("deleted")
                .action(remove(cx)),
            ServerStatus::Incompatible(reason) => {
                let (title, detail) = text::incompatibility(*reason);
                let detail = self.incompatibility_detail(*reason, detail, cx);
                StatePanel::new(title, detail)
                    .icon(Icon::Warning)
                    .selector("incompatible")
                    .action(check(text::CHECK_AGAIN, cx))
                    .action(remove(cx))
            }
            ServerStatus::OtherServer => {
                StatePanel::new(text::OTHER_SERVER_TITLE, text::OTHER_SERVER)
                    .icon(Icon::Warning)
                    .selector("other-server")
                    .action(check(text::CHECK_AGAIN, cx))
                    .action(remove(cx))
            }
        };
        // A failure of the last action is shown with every state except a wait that the
        // application repeats by itself.
        let shows_error = !matches!(server.status, ServerStatus::Connecting);
        panel.error(error.filter(|_| shows_error))
    }

    /// The side panel shows the member list with this account, which states the refusal of
    /// its conversation at its row.
    fn refusal_at_member(&self, view: &ShellView, account_id: AccountId) -> bool {
        let Some(lists) = view.lists() else {
            return false;
        };
        let Some(workspace) = &lists.workspace else {
            return false;
        };
        self.members_open
            && lists.call.is_none()
            && lists.notices.is_none()
            && workspace.thread.is_none()
            && workspace.pins.is_none()
            && workspace.transcript.is_none()
            && workspace.search.is_none()
            && matches!(&workspace.members, Members::Loaded(members)
                if members.iter().any(|member| member.account_id == account_id))
    }

    /// The members of the shown workspace in the side panel. Without a workspace the panel
    /// states that there is none, so that the main area keeps its width (ADR 0730).
    fn render_members(
        &mut self,
        lists: &ServerLists,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let workspace = lists.workspace.as_ref();
        let colors = theme.colors;
        let faces = self.shown_faces();
        let header = SideHeader::new(text::MEMBERS)
            .title_bar()
            .icon(Icon::People)
            .note(
                workspace
                    .map(|workspace| workspace.name.clone())
                    .unwrap_or_default(),
            )
            .trailing(
                Button::new("close-members")
                    .kind(ButtonKind::Ghost)
                    .icon(Icon::X)
                    .tooltip(text::CLOSE_MEMBERS)
                    .selector("close-members")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.members_open = false;
                        cx.notify();
                    })),
            );
        let note = |text: &'static str| {
            div()
                .px(u(14.0))
                .pt(u(12.0))
                .typeface(theme.types.small, theme)
                .ink(colors.ink_2)
                .child(text)
        };
        let Some(members) = workspace.map(|workspace| &workspace.members) else {
            return self.side_panel(
                header,
                note(text::NO_WORKSPACE_MEMBERS)
                    .debug_selector(|| "no-workspace-members".to_owned())
                    .into_any_element(),
                theme,
            );
        };
        let body: AnyElement = match members {
            Members::NotLoaded | Members::Loading => note(text::MEMBERS_LOADING).into_any_element(),
            Members::Failed => div()
                .flex()
                .flex_col()
                .gap(u(8.0))
                .px(u(14.0))
                .pt(u(12.0))
                .child(Notice::error(text::MEMBERS_FAILED).selector("members-failed"))
                .child(
                    div().flex().child(
                        Button::new("reload-members")
                            .label(text::TRY_AGAIN)
                            .icon(Icon::Retry)
                            .selector("reload-members")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.state.update(cx, |state, cx| state.reload_members(cx));
                            })),
                    ),
                )
                .into_any_element(),
            Members::Loaded(members) => {
                let server_id = self
                    .view
                    .as_ref()
                    .and_then(|view| view.selected.as_ref())
                    .map(|server| server.id);
                let refused = lists.conversation_refused.clone();
                let asked = lists.conversation_asked;
                let rows = members.iter().enumerate().map(|(index, member)| {
                    let detail: SharedString = member.roles.join(" · ").into();
                    let account_id = member.account_id;
                    let write = (!member.is_self).then(|| {
                        div()
                            .flex_none()
                            .debug_selector(move || format!("message-member:{account_id}"))
                            .child(
                                Button::new(("message-member", index))
                                    .kind(ButtonKind::Ghost)
                                    .icon(Icon::Chat)
                                    .tooltip(text::message_person(&member.name))
                                    .accessible_label(text::message_person(&member.name))
                                    .disabled(asked == Some(account_id))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        let Some(server_id) = server_id else {
                                            return;
                                        };
                                        this.state.update(cx, |state, cx| {
                                            state.open_conversation(server_id, account_id, cx);
                                        });
                                    })),
                            )
                    });
                    let refusal = refused
                        .as_ref()
                        .filter(|refusal| refusal.account_id == account_id)
                        .map(|refusal| {
                            div()
                                .debug_selector(|| "conversation-refused".to_owned())
                                .typeface(theme.types.small, theme)
                                .line_height(u(17.0))
                                .ink(colors.cinnabar_ink)
                                .child(SharedString::from(refusal.reason.clone()))
                        });
                    div()
                        .id(("member", index))
                        .role(Role::ListItem)
                        .aria_label(member.name.clone())
                        .debug_selector({
                            let account_id = member.account_id;
                            move || format!("member-row:{account_id}")
                        })
                        .flex()
                        .flex_none()
                        .gap(u(10.0))
                        .pt(u(7.0))
                        .pb(u(8.0))
                        .pl(u(14.0))
                        .pr(u(12.0))
                        .when(index > 0, |row| {
                            row.border_t_1().border_color(Hsla::from(colors.rule_soft))
                        })
                        .child(div().mt(u(2.0)).child(super::people::avatar_of(
                            &faces,
                            member.account_id,
                            &member.name,
                            member.pictured,
                            30.0,
                        )))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(u(7.0))
                                        .line_height(u(19.0))
                                        .child(
                                            div()
                                                .min_w_0()
                                                .truncate()
                                                .typeface(theme.types.name, theme)
                                                .line_height(u(19.0))
                                                .ink(colors.ink)
                                                .child(member.name.clone()),
                                        )
                                        .when(member.is_self, |line| {
                                            line.child(
                                                div()
                                                    .flex_none()
                                                    .typeface(theme.types.small, theme)
                                                    .ink(colors.ink_2)
                                                    .child(text::YOU),
                                            )
                                        }),
                                )
                                .when(!detail.is_empty(), |column| {
                                    column.child(
                                        div()
                                            .typeface(theme.types.small, theme)
                                            .line_height(u(18.0))
                                            .ink(colors.ink_2)
                                            .truncate()
                                            .child(detail),
                                    )
                                })
                                .children(refusal),
                        )
                        .children(write)
                });
                ScrollArea::new("member-list", &self.members_scroll, text::MEMBERS)
                    .child(SectionLabel::new(text::MEMBERS).count(members.len()))
                    .children(rows)
                    .child(div().flex_none().h(u(4.0)))
                    .into_any_element()
            }
        };
        self.side_panel(header, body, theme)
    }

    /// The side panel of the members with its header and its body.
    fn side_panel(&self, header: SideHeader, body: AnyElement, theme: &Theme) -> AnyElement {
        let colors = theme.colors;
        div()
            .id("side-panel")
            .role(Role::Complementary)
            .aria_label(text::MEMBERS)
            .debug_selector(|| "side-panel".to_owned())
            .flex()
            .flex_col()
            .flex_none()
            .w(u(theme.metrics.side_panel_width))
            .h_full()
            .bg(Hsla::from(colors.plaster))
            .border_l_1()
            .border_color(Hsla::from(colors.rule))
            .child(header)
            .child(body)
            .into_any_element()
    }
}

/// The mark of a recording that runs: the mark with its time on cinnabar, and beside it the
/// statement that every person in the call sees it (ADR 0601).
fn recording_mark(mark: SharedString, theme: &Theme) -> AnyElement {
    recording_mark_in(mark, false, false, theme)
}

/// The Stop action of a running recording: its label in the wide form, and the text of its
/// tooltip and accessible label. While the recording is stopping, the disabled action states it.
fn stop_action(stopping: bool) -> (&'static str, &'static str) {
    if stopping {
        (text::RECORDING_STOPS, text::RECORDING_STOPS)
    } else {
        (text::STOP, text::STOP_RECORDING)
    }
}

/// The mark of the recording of the call of this device. Its time part is as wide as the
/// widest text that it shows, "REC starting…" or "REC 00:00:00", in the type of the mark:
/// both texts are laid out in it without height.
/// With `shrinks` the words after the time end with an ellipsis where the place is too
/// narrow, and the time keeps its width.
fn recording_mark_reserved(mark: SharedString, shrinks: bool, theme: &Theme) -> AnyElement {
    recording_mark_in(mark, true, shrinks, theme)
}

fn recording_mark_in(
    mark: SharedString,
    reserved: bool,
    shrinks: bool,
    theme: &Theme,
) -> AnyElement {
    let colors = theme.colors;
    let widths = [
        format!("{} {}", text::RECORDING_MARK, text::RECORDING_STARTS),
        text::recording_time(std::time::Duration::ZERO),
    ];
    let text = div().flex().flex_col().child(mark).when(reserved, |text| {
        text.children(
            widths
                .into_iter()
                .map(|width| div().h_0().overflow_hidden().child(width)),
        )
    });
    div()
        .id("recording-indicator")
        .role(Role::Status)
        .aria_label(text::RECORDING_RUNS)
        .debug_selector(|| "recording-indicator".to_owned())
        .tooltip(tooltip(text::RECORDING_TIME))
        .flex()
        .map(|indicator| {
            if shrinks {
                indicator.min_w_0()
            } else {
                indicator.flex_none()
            }
        })
        .items_center()
        .child(
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(u(7.0))
                .h(u(28.0))
                .pl(u(8.0))
                .pr(u(9.0))
                .rounded_l(u(theme.metrics.radius))
                .bg(Hsla::from(colors.cinnabar))
                .typeface(theme.types.mono_mark, theme)
                .text_size(u(13.0))
                .ink(colors.on_terracotta)
                .whitespace_nowrap()
                .child(
                    div()
                        .size(u(9.0))
                        .rounded_full()
                        .bg(Hsla::from(colors.on_terracotta)),
                )
                .child(text),
        )
        .child(
            div()
                .flex()
                .items_center()
                .h(u(28.0))
                .px(u(9.0))
                .rounded_r(u(theme.metrics.radius))
                .border_1()
                .border_l_0()
                .border_color(Hsla::from(colors.cinnabar_wash_rule))
                .bg(Hsla::from(colors.cinnabar_wash))
                .typeface(theme.types.small, theme)
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .ink(colors.cinnabar_ink)
                .whitespace_nowrap()
                .map(|words| {
                    if shrinks {
                        words
                            .min_w_0()
                            .overflow_hidden()
                            .child(div().min_w_0().truncate().child(text::RECORDING_VISIBLE))
                    } else {
                        words.child(text::RECORDING_VISIBLE)
                    }
                }),
        )
        .into_any_element()
}

/// The state of the call of a voice channel in the header of the main area, for a person
/// outside the call: recording, meeting mode, the lock and the count.
fn voice_header_states(voice: &VoiceState, theme: &Theme) -> Vec<AnyElement> {
    let mut states = Vec::new();
    if voice.recording {
        states.push(recording_mark(text::RECORDING_MARK.into(), theme));
    }
    states.extend(voice_room_states(voice, theme));
    states
}

/// The state of the room of a voice channel: meeting mode, the lock and the count.
fn voice_room_states(voice: &VoiceState, theme: &Theme) -> Vec<AnyElement> {
    let colors = theme.colors;
    let state = |shown: Icon, label: SharedString| {
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(u(6.0))
            .h(u(28.0))
            .px(u(4.0))
            .typeface(theme.types.control_strong, theme)
            .ink(colors.ink)
            .whitespace_nowrap()
            .child(icon(shown, 15.0, colors.ink_2))
            .child(label)
            .into_any_element()
    };
    let mut states = Vec::new();
    if voice.meeting {
        states.push(state(Icon::Hand, text::MEETING_MODE.into()));
    }
    if voice.locked {
        states.push(state(Icon::Lock, text::LOCKED_STATE.into()));
    }
    if let (Some(limit), false) = (voice.user_limit, voice.hidden) {
        states.push(state(
            Icon::People,
            text::occupancy(voice.occupants.len(), limit, voice.full).into(),
        ));
    }
    states
}

impl Shell {
    /// The call of this device offers "Record" now: a person who may record sees it while
    /// no recording starts or runs.
    pub(crate) fn records_now(&self, view: &ShellView) -> bool {
        view.lists()
            .and_then(|lists| lists.call.as_ref())
            .and_then(|panel| panel.recording())
            .is_some_and(|recording| {
                recording.may_record
                    && !recording.busy
                    && matches!(
                        recording.state,
                        RecordingShown::None | RecordingShown::Failed(_)
                    )
            })
    }

    /// Opens the languages of the speech of a new recording at the button that records.
    pub fn open_record_languages(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // A click on the button of open languages closes them: the press closed them, and
        // the click must not open them again.
        if std::mem::take(&mut self.record_language_closed_by_press) {
            return;
        }
        self.recording_details_open = false;
        self.open_layer(Layer::RecordLanguage, window, cx);
    }

    /// The languages of the speech of a new recording (ADR 0603): the detection of the
    /// language of each sentence first, then each language that the speech model knows.
    /// A choice starts the recording with it.
    fn render_record_languages(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let choice = |id: (&'static str, usize),
                      title: &'static str,
                      language: Option<&'static str>,
                      selector: String,
                      this: &mut Self,
                      cx: &mut Context<Self>| {
            div()
                .flex_none()
                .debug_selector(move || selector)
                .child(
                    crate::ui::MenuItem::new(id, title).on_click(this.call_listener(
                        cx,
                        move |this, _, window, cx| {
                            this.close_layer(window, cx);
                            let language = language.map(str::to_owned);
                            this.state
                                .update(cx, |state, cx| state.start_recording(language, cx));
                        },
                    )),
                )
                .into_any_element()
        };
        let mut items: Vec<AnyElement> = vec![
            crate::ui::menu_label(text::transcripts::LANGUAGE_MENU, false, cx).into_any_element(),
        ];
        items.push(choice(
            ("record-language", 0),
            text::transcripts::DETECT,
            None,
            "record-language:auto".to_owned(),
            self,
            cx,
        ));
        items.push(
            crate::ui::menu_label(text::transcripts::ONE_LANGUAGE, false, cx).into_any_element(),
        );
        let mut languages: Vec<AnyElement> = Vec::new();
        for (index, (code, name)) in text::transcripts::LANGUAGES.iter().enumerate() {
            languages.push(choice(
                ("record-language", index + 1),
                name,
                Some(code),
                format!("record-language:{code}"),
                self,
                cx,
            ));
        }
        items.push(
            div()
                .id("record-languages")
                .debug_selector(|| "record-language-list".to_owned())
                .flex()
                .flex_col()
                .max_h(u(300.0))
                .overflow_y_scroll()
                .children(languages)
                .into_any_element(),
        );
        div()
            .absolute()
            .left_0()
            .top(u(32.0))
            .child(deferred(
                gpui::anchored().snap_to_window().child(
                    crate::ui::Menu::new(
                        "record-language-menu",
                        text::transcripts::LANGUAGE_MENU,
                        self.layer_focus(),
                    )
                    .width(300.0)
                    .selector("record-language-menu")
                    .on_close(Self::handler(cx, |this, window, cx| {
                        this.record_language_closed_by_press = !window.last_input_was_keyboard();
                        this.close_layer(window, cx);
                    }))
                    .children(items)
                    .child(div().h(u(4.0))),
                ),
            ))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stop_action_of_a_stopping_recording_states_that_it_stops() {
        assert_eq!(stop_action(false), (text::STOP, text::STOP_RECORDING));
        assert_eq!(
            stop_action(true),
            (text::RECORDING_STOPS, text::RECORDING_STOPS)
        );
    }
}
