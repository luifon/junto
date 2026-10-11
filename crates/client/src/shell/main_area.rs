//! The main area: the header of the selected channel, the statement of what this version
//! shows for it, the state of a server that has no lists, the recovery notices, and the
//! side panel with the members of the workspace.

use gpui::{
    AnyElement, BoxShadow, Context, Hsla, InteractiveElement, IntoElement, ParentElement, Role,
    SharedString, StatefulInteractiveElement, Styled, StyledText, Window, deferred, div,
    prelude::FluentBuilder, px,
};
use junto_client_core::protocol::structure::ChannelKind;

use super::{Layer, Shell};
use crate::assets::Icon;
use junto_media::calls::Phase;

use crate::state::view::{
    CallPanel, ChannelRow, Content, Members, RecordingControl, RecordingShown, Relation,
    SelectedServer, ServerStatus, ShellView, VoiceState, WorkspaceView,
};
use crate::text;
use crate::theme::{Theme, theme, u};
use crate::ui::{
    Button, ButtonKind, Flag, FlagKind, Mosaic, MosaicKind, Notice, ScrollArea, SectionLabel,
    SideHeader, StatePanel, StyleExt, float_shadow, icon, tooltip,
};

/// The time of a recovery as the notice shows it: the date and the time in UTC.
fn recovery_time(notice: &junto_client_core::protocol::account::RecoveryNotice) -> String {
    let at = notice.performed_at.to_offset(time::UtcOffset::UTC);
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
        let workspace = view.workspace();
        let channel = workspace.and_then(|workspace| workspace.channel.as_ref());
        // The call of this device, when its voice channel is the selected one.
        let call = workspace
            .zip(channel)
            .and_then(|(workspace, channel)| Some((channel, workspace.call.as_ref()?)));
        let members_open = self.members_open && workspace.is_some() && call.is_none();

        let (body, side): (AnyElement, Option<AnyElement>) = match (&view.selected, workspace) {
            (None, _) => (
                StatePanel::new(text::NO_SERVER_TITLE, text::NO_SERVER)
                    .selector("no-server")
                    .into_any_element(),
                None,
            ),
            (Some(_), Some(shown)) => match (call, channel) {
                (Some((channel, panel)), _) => {
                    self.render_call(view, channel, panel, narrow, window, cx)
                }
                (None, Some(channel)) => (
                    self.render_channel_body(channel).into_any_element(),
                    members_open.then(|| self.render_members(shown, &theme, cx)),
                ),
                (None, None) => (
                    StatePanel::new(text::NO_CHANNEL_TITLE, text::NO_CHANNELS)
                        .selector("no-channel")
                        .into_any_element(),
                    members_open.then(|| self.render_members(shown, &theme, cx)),
                ),
            },
            (Some(server), None) => (
                match server.content {
                    Content::NoWorkspace => {
                        StatePanel::new(text::NO_CHANNEL_TITLE, text::NO_WORKSPACES)
                            .selector("no-workspace")
                            .into_any_element()
                    }
                    _ => self.render_server_state(server, cx).into_any_element(),
                },
                None,
            ),
        };

        let in_call = call.is_some();
        let split = div()
            .relative()
            .flex()
            .flex_1()
            .min_h_0()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .children(self.render_notices(view, &theme, cx))
                    .child(body),
            )
            .when_some(side, |split, side| {
                if narrow {
                    // The panel lies over the content, with a flat cover behind it.
                    split
                        .child(Self::side_cover(&theme, cx, move |this, _| {
                            if in_call {
                                this.call_panel_open = false;
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
            .child(self.render_main_header(
                view,
                channel,
                call.map(|(_, panel)| panel),
                narrow,
                &theme,
                cx,
            ))
            .children(
                call.and_then(|(_, panel)| panel.room.meeting.as_ref())
                    .map(|meeting| self.render_meeting_bar(meeting, &theme, cx)),
            )
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
        let title: Option<(Icon, SharedString)> = channel.map(|channel| {
            (
                match channel.kind {
                    ChannelKind::Text => Icon::Hash,
                    ChannelKind::Voice => Icon::Speaker,
                },
                channel.name.clone().into(),
            )
        });
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
            if panel.phase == Phase::Reconnecting {
                after.extend(Self::render_call_interruption(theme));
                after.push(div().flex_1().into_any_element());
            } else {
                // The recording of the call, and then the state of the room.
                after.push(
                    div()
                        .flex_none()
                        .w(px(1.0))
                        .h(u(20.0))
                        .bg(Hsla::from(colors.rule))
                        .into_any_element(),
                );
                after.extend(self.render_recording_control(&panel.recording, theme, cx));
                after.push(div().flex_1().into_any_element());
                after.extend(self.render_room_controls(&panel.room, theme, cx));
            }
            if narrow {
                after.extend(self.render_call_panel_buttons(cx));
            }
        } else {
            after.push(div().flex_1().into_any_element());
        }
        div()
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
            .when(view.workspace().is_some() && call.is_none(), |header| {
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
                            cx.notify();
                        })),
                )
            })
    }

    /// The recording of the call of this device in the header of its voice channel
    /// (ADR 0738): the mark with its time and the action that stops it, the action that
    /// starts one, and what happened to the last one.
    fn render_recording_control(
        &mut self,
        control: &RecordingControl,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let colors = theme.colors;
        let record = |title: &'static str, cx: &mut Context<Self>| {
            Button::new("start-recording")
                .icon(Icon::Record)
                .icon_colour(colors.cinnabar)
                .label(text::RECORD)
                .tooltip(title)
                .accessible_label(title)
                .disabled(control.busy)
                .selector("start-recording")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.recording_details_open = false;
                    this.state.update(cx, |state, cx| state.start_recording(cx));
                }))
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
                .ink(colors.ink_2)
                .child(sentence)
                .into_any_element()
        };
        let forbidden = || {
            vec![
                Button::new("start-recording")
                    .icon(Icon::Record)
                    .label(text::RECORD)
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
            RecordingShown::Starting => vec![recording_mark(
                format!("{} {}", text::RECORDING_MARK, text::RECORDING_STARTS).into(),
                theme,
            )],
            RecordingShown::Running { elapsed, stopping } => {
                let mark: SharedString = match elapsed {
                    Some(elapsed) => text::recording_time(*elapsed).into(),
                    None => text::RECORDING_MARK.into(),
                };
                let mut shown = vec![recording_mark(mark, theme)];
                if control.may_record {
                    shown.push(
                        Button::new("stop-recording")
                            .icon(Icon::Stop)
                            .icon_colour(colors.cinnabar)
                            .label(if *stopping {
                                text::RECORDING_STOPS
                            } else {
                                text::STOP
                            })
                            .tooltip(text::STOP_RECORDING)
                            .accessible_label(text::STOP_RECORDING)
                            .disabled(*stopping || control.busy)
                            .selector("stop-recording")
                            .on_click(cx.listener(|this, _, _, cx| {
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
                        .flex_none()
                        .child(
                            Flag::new(FlagKind::Error, text::recording_flag(problem))
                                .icon(Icon::Warning),
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

    /// The body of a channel that has no content in this version: a text channel, and the
    /// chat of a voice channel that this device is not in.
    fn render_channel_body(&self, channel: &ChannelRow) -> impl IntoElement + use<> {
        match &channel.voice {
            None => StatePanel::new(
                text::MESSAGES_NOT_AVAILABLE_TITLE,
                text::MESSAGES_NOT_AVAILABLE,
            )
            .selector("messages-not-available"),
            Some(_) => StatePanel::new(
                text::MESSAGES_NOT_AVAILABLE_TITLE,
                text::VOICE_CHAT_NOT_AVAILABLE,
            )
            .selector("voice-chat-not-available"),
        }
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
                                    this.state.update(cx, |state, _| state.stop_share());
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
        let mut notices: Vec<AnyElement> = about_call().collect();
        let Some(server) = &view.selected else {
            return notices;
        };
        let server_id = server.id;
        notices.extend(
            server
                .recoveries
                .iter()
                .enumerate()
                .map(|(index, recovery)| {
                    let recovery_id = recovery.id;
                    let sending = pending.acknowledging.contains(&recovery_id);
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
        if let Some(error) = pending.acknowledge_failed.get(&server_id) {
            notices.push(
                div()
                    .flex_none()
                    .px(u(16.0))
                    .pt(u(10.0))
                    .child(Notice::error(error.clone()).selector("recovery-error"))
                    .into_any_element(),
            );
        }
        let _ = theme;
        notices
    }

    /// The state of a server that shows no lists, with the actions that the state accepts.
    fn render_server_state(
        &mut self,
        server: &SelectedServer,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let server_id = server.id;
        let busy = server.busy
            || self
                .state
                .read(cx)
                .pending()
                .cancelling
                .contains(&server_id);
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
            ServerStatus::Incompatible(reason) => {
                let (title, detail) = text::incompatibility(*reason);
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

    fn render_members(
        &mut self,
        workspace: &WorkspaceView,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = theme.colors;
        let header = SideHeader::new(text::MEMBERS)
            .icon(Icon::People)
            .note(workspace.name.clone())
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
        let body: AnyElement = match &workspace.members {
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
                let rows = members.iter().enumerate().map(|(index, member)| {
                    let detail: SharedString = member.roles.join(" · ").into();
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
                        .child(div().mt(u(2.0)).child(Mosaic::new(
                            MosaicKind::Large,
                            member.account_id.as_uuid().as_u128(),
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
                                }),
                        )
                });
                ScrollArea::new("member-list", &self.members_scroll, text::MEMBERS)
                    .child(SectionLabel::new(text::MEMBERS).count(members.len()))
                    .children(rows)
                    .child(div().flex_none().h(u(4.0)))
                    .into_any_element()
            }
        };
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
    let colors = theme.colors;
    div()
        .id("recording-indicator")
        .role(Role::Status)
        .aria_label(text::RECORDING_RUNS)
        .debug_selector(|| "recording-indicator".to_owned())
        .tooltip(tooltip(text::RECORDING_TIME))
        .flex()
        .flex_none()
        .items_center()
        .child(
            div()
                .flex()
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
                .child(mark),
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
                .child(text::RECORDING_VISIBLE),
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
