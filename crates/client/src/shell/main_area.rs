//! The main area: the header of the selected channel, the statement of what this version
//! shows for it, the state of a server that has no lists, the recovery notices, and the
//! side panel with the members of the workspace.

use gpui::{
    AnyElement, BoxShadow, Context, Hsla, InteractiveElement, IntoElement, MouseButton,
    ParentElement, Role, SharedString, StatefulInteractiveElement, Styled, Window, div,
    prelude::FluentBuilder, px,
};
use junto_client_core::protocol::structure::ChannelKind;

use super::{Layer, Shell};
use crate::assets::Icon;
use crate::state::view::{
    ChannelRow, Content, Members, SelectedServer, ServerStatus, ShellView, VoiceState,
    WorkspaceView,
};
use crate::text;
use crate::theme::{Theme, theme, u};
use crate::ui::{
    Button, ButtonKind, Mosaic, MosaicKind, Notice, ScrollArea, SectionLabel, SideHeader,
    StatePanel, StyleExt, float_shadow, icon,
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
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = theme(cx);
        let workspace = view.workspace();
        let channel = workspace.and_then(|workspace| workspace.channel.as_ref());
        let members_open = self.members_open && workspace.is_some();

        let body: AnyElement = match (&view.selected, workspace) {
            (None, _) => StatePanel::new(text::NO_SERVER_TITLE, text::NO_SERVER)
                .selector("no-server")
                .into_any_element(),
            (Some(_), Some(_)) => match channel {
                Some(channel) => self.render_channel_body(channel, &theme).into_any_element(),
                None => StatePanel::new(text::NO_CHANNEL_TITLE, text::NO_CHANNELS)
                    .selector("no-channel")
                    .into_any_element(),
            },
            (Some(server), None) => match server.content {
                Content::NoWorkspace => {
                    StatePanel::new(text::NO_CHANNEL_TITLE, text::NO_WORKSPACES)
                        .selector("no-workspace")
                        .into_any_element()
                }
                _ => self.render_server_state(server, cx).into_any_element(),
            },
        };

        let side = workspace
            .filter(|_| members_open)
            .map(|workspace| self.render_members(workspace, &theme, cx));
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
                    .children(self.render_recoveries(view, &theme, cx))
                    .child(body),
            )
            .when_some(side, |split, side| {
                if narrow {
                    // The panel lies over the content, with a flat cover behind it.
                    split
                        .child(
                            div()
                                .id("side-cover")
                                .absolute()
                                .inset_0()
                                .occlude()
                                .bg(Hsla::from(theme.colors.scrim_soft))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.members_open = false;
                                        cx.notify();
                                    }),
                                ),
                        )
                        .child(
                            div()
                                .absolute()
                                .top_0()
                                .bottom_0()
                                .right_0()
                                .flex()
                                .occlude()
                                .shadow(float_shadow(theme.colors.shadow_deep, 0.0, 26.0))
                                .child(side),
                        )
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
            .child(self.render_main_header(view, channel, members_open, &theme, cx))
            .child(split)
    }

    fn render_main_header(
        &mut self,
        view: &ShellView,
        channel: Option<&ChannelRow>,
        members_open: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = theme.colors;
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
        div()
            .id("main-header")
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .gap(u(10.0))
            .h(u(theme.metrics.header_height))
            .pl(u(16.0))
            .pr(u(14.0))
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
            .child(div().flex_1())
            .when_some(voice, |header, voice| {
                header.children(voice_header_states(voice, theme))
            })
            .when(view.workspace().is_some(), |header| {
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

    fn render_channel_body(&self, channel: &ChannelRow, theme: &Theme) -> impl IntoElement + use<> {
        match &channel.voice {
            None => StatePanel::new(
                text::MESSAGES_NOT_AVAILABLE_TITLE,
                text::MESSAGES_NOT_AVAILABLE,
            )
            .selector("messages-not-available")
            .into_any_element(),
            Some(voice) => {
                let inside: SharedString = if voice.hidden {
                    text::OCCUPANTS_HIDDEN.into()
                } else if voice.occupants.is_empty() {
                    text::NOBODY_INSIDE.into()
                } else {
                    text::people_inside(voice.occupants.len()).into()
                };
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .child(
                        StatePanel::new(
                            text::CALLS_NOT_AVAILABLE_TITLE,
                            format!("{} {inside}", text::CALLS_NOT_AVAILABLE),
                        )
                        .selector("calls-not-available"),
                    )
                    .typeface(theme.types.body, theme)
                    .into_any_element()
            }
        }
    }

    /// The notices of recoveries that this device has not acknowledged (ADR 0118). A
    /// notice goes only when the person presses its button.
    fn render_recoveries(
        &mut self,
        view: &ShellView,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let Some(server) = &view.selected else {
            return Vec::new();
        };
        let server_id = server.id;
        let pending = self.state.read(cx).pending().clone();
        let mut notices: Vec<AnyElement> = server
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
            })
            .collect();
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

/// The state of the call of a voice channel in the header of the main area: recording,
/// meeting mode, the lock and the count.
fn voice_header_states(voice: &VoiceState, theme: &Theme) -> Vec<AnyElement> {
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
    if voice.recording {
        states.push(
            div()
                .id("recording-indicator")
                .role(Role::Status)
                .aria_label(text::RECORDING_RUNS)
                .debug_selector(|| "recording-indicator".to_owned())
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
                        .child(
                            div()
                                .size(u(9.0))
                                .rounded_full()
                                .bg(Hsla::from(colors.on_terracotta)),
                        )
                        .child(text::RECORDING_MARK),
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
                .into_any_element(),
        );
    }
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
