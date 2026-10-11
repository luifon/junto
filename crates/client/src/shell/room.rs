//! The rules of the room of a call on screen (ADR 0739): meeting mode and the lock in the
//! header of the voice channel, the bar of meeting mode, the settings of the room, the
//! invitation of a member, the request to enter a room that is locked or full, the menu of
//! a person in a voice channel, and what the server tells or refuses.
//!
//! Every function here draws from the derived view. Each action asks the server and
//! changes nothing on screen by itself: the change arrives with the state of the server,
//! and a refusal is shown as a notice.

use gpui::{
    AnyElement, Context, Div, Hsla, InteractiveElement, IntoElement, ParentElement, Role,
    SharedString, StatefulInteractiveElement, Styled, StyledText, Window, anchored, deferred, div,
    prelude::FluentBuilder,
};
use junto_client_core::protocol::call::UpdateCallSettings;
use junto_client_core::protocol::ids::{AccountId, ChannelId, ServerId};

use super::{Layer, Shell};
use crate::assets::Icon;
use crate::state::view::{
    CallPanel, ChannelRow, Closed, EntryState, MeetingStart, MeetingView, OutsideMember,
    PersonMenu, Relation, RoomView, ShellView, VoiceState,
};
use crate::text;
use crate::theme::{Theme, theme, u};
use crate::ui::{
    Button, ButtonKind, Field, FieldKind, Menu, MenuItem, Mosaic, MosaicKind, Notice, SectionLabel,
    Stepper, StyleExt, Toggle, VolumeSlider, float_shadow, icon, ledge, menu_label, menu_separator,
    tooltip,
};

/// The largest user limit of a voice channel (ADR 0514).
const USER_LIMIT_MAX: u32 = 99;

/// The menu of a person that is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PersonMenuOpen {
    pub channel_id: ChannelId,
    pub account_id: AccountId,
    /// The menu lists the voice channels that the person can be moved to.
    pub moving: bool,
}

/// A row of settings with a title, a sentence under it, and a control at its end.
pub(crate) fn option_row(
    title: &'static str,
    sentence: &'static str,
    control: AnyElement,
    theme: &Theme,
) -> Div {
    let colors = theme.colors;
    div()
        .flex()
        .flex_none()
        .items_start()
        .gap(u(10.0))
        .pt(u(5.0))
        .pb(u(6.0))
        .px(u(14.0))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .typeface(theme.types.control, theme)
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .line_height(u(19.0))
                        .ink(colors.ink)
                        .child(title),
                )
                .child(
                    div()
                        .typeface(theme.types.small, theme)
                        .line_height(u(17.0))
                        .ink(colors.ink_2)
                        .child(sentence),
                ),
        )
        .child(div().flex_none().mt(u(1.0)).child(control))
}

/// Meeting mode as its switch shows it: whether it is on, why the person cannot change it,
/// and what the switch does.
fn meeting_switch(room: &RoomView) -> (bool, Option<SharedString>, SharedString) {
    match &room.meeting {
        Some(meeting) if meeting.by_self => (true, None, text::MEETING_TURN_OFF.into()),
        Some(meeting) => {
            let reason: SharedString = text::meeting_run_by(meeting.presenter.as_deref()).into();
            (true, Some(reason.clone()), reason)
        }
        None => {
            let reason = match room.meeting_start {
                MeetingStart::Possible => None,
                MeetingStart::NotSharing => Some(text::MEETING_NEEDS_SHARE),
                MeetingStart::NoPermission => Some(text::MEETING_NO_PERMISSION),
                MeetingStart::MicrophoneBlocked => Some(text::MEETING_WHILE_BLOCKED),
            };
            (
                false,
                reason.map(SharedString::from),
                reason.unwrap_or(text::MEETING_MODE_HELP).into(),
            )
        }
    }
}

const fn on_off(on: bool) -> &'static str {
    if on { text::ON } else { text::OFF }
}

/// The user limit after one step down or up. `None` as a result: the step is not possible.
/// A limit of one that is lowered is removed, and a channel without a limit gets the number
/// of people inside as its first limit.
fn stepped_limit(limit: Option<u32>, inside: usize, up: bool) -> Option<Option<u32>> {
    match (limit, up) {
        (None, false) => None,
        (None, true) => Some(Some(
            u32::try_from(inside)
                .unwrap_or(USER_LIMIT_MAX)
                .clamp(1, USER_LIMIT_MAX),
        )),
        (Some(1), false) => Some(None),
        (Some(limit), false) => Some(Some(limit - 1)),
        (Some(limit), true) if limit >= USER_LIMIT_MAX => None,
        (Some(limit), true) => Some(Some(limit + 1)),
    }
}

impl Shell {
    /// The controls of the room in the header of its voice channel: meeting mode, the
    /// lock, and the count of the people inside.
    pub(crate) fn render_room_controls(
        &mut self,
        room: &RoomView,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let colors = theme.colors;
        let (meeting_on, meeting_held, meeting_help) = meeting_switch(room);
        let locked = room.locked;
        let mut controls = vec![
            Button::new("meeting-toggle")
                .icon(Icon::Hand)
                .label(text::MEETING_MODE)
                .tag(on_off(meeting_on), meeting_on)
                .tooltip(meeting_help.clone())
                // The state, and why the person cannot change it, for a screen reader.
                .accessible_label(match &meeting_held {
                    Some(reason) => {
                        format!("{} · {}. {reason}", text::MEETING_MODE, on_off(meeting_on))
                    }
                    None => format!("{} · {}", text::MEETING_MODE, on_off(meeting_on)),
                })
                .disabled(meeting_held.is_some())
                .selector("meeting-toggle")
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.state
                        .update(cx, |state, cx| state.set_meeting(!meeting_on, cx));
                }))
                .into_any_element(),
            Button::new("lock-toggle")
                .icon(if locked { Icon::Lock } else { Icon::Unlock })
                .label(text::LOCK_ROOM)
                .tag(on_off(locked), locked)
                .tooltip(if room.allowed.lock {
                    text::LOCK_ROOM_HELP
                } else {
                    text::LOCK_NO_PERMISSION
                })
                .accessible_label(if room.allowed.lock {
                    format!("{} · {}", text::LOCK_ROOM, on_off(locked))
                } else {
                    format!(
                        "{} · {}. {}",
                        text::LOCK_ROOM,
                        on_off(locked),
                        text::LOCK_NO_PERMISSION
                    )
                })
                .disabled(!room.allowed.lock)
                .selector("lock-toggle")
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.state
                        .update(cx, |state, cx| state.set_locked(!locked, cx));
                }))
                .into_any_element(),
        ];
        if let Some(limit) = room.user_limit {
            let count = text::occupancy(room.inside, limit, false);
            controls.push(
                div()
                    .id("room-count")
                    .role(Role::Status)
                    .aria_label(text::occupancy_help(room.inside, limit))
                    .debug_selector(|| "room-count".to_owned())
                    .tooltip(tooltip(text::occupancy_help(room.inside, limit)))
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(u(6.0))
                    .h(u(28.0))
                    .px(u(4.0))
                    .typeface(theme.types.control_strong, theme)
                    .ink(colors.ink)
                    .whitespace_nowrap()
                    .child(icon(Icon::People, 15.0, colors.ink_2))
                    .child(count)
                    .into_any_element(),
            );
        }
        controls
    }

    /// The bar under the header of a call in which meeting mode is on (ADR 0505): who runs
    /// it, and for the presenter the raised hands, for a participant the statement that
    /// the presenter lets the person speak.
    pub(crate) fn render_meeting_bar(
        &mut self,
        meeting: &MeetingView,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = theme.colors;
        let line = if meeting.by_self {
            format!("{}{}", text::MEETING_IS_ON, text::YOU_RUN_IT)
        } else {
            format!(
                "{}{}",
                text::MEETING_IS_ON,
                text::runs_it(meeting.presenter.as_deref())
            )
        };
        let strong = |sentence: String, strong: usize| {
            StyledText::new(sentence).with_highlights([(
                0..strong,
                gpui::HighlightStyle {
                    font_weight: Some(gpui::FontWeight::BOLD),
                    color: Some(Hsla::from(colors.call_text_hi)),
                    ..gpui::HighlightStyle::default()
                },
            )])
        };
        let request = div().flex().flex_none().items_center().gap(u(8.0));
        let request: Option<AnyElement> = if meeting.by_self {
            match (meeting.hands.first(), meeting.waiting.first()) {
                (Some((account_id, name)), _) => {
                    let account_id = *account_id;
                    let name = name
                        .clone()
                        .unwrap_or_else(|| text::UNKNOWN_PERSON.to_owned());
                    let more = meeting.hands.len() - 1;
                    Some(
                        request
                            .child(
                                Mosaic::new(
                                    MosaicKind::Large,
                                    account_id.as_uuid().as_u128(),
                                    22.0,
                                )
                                .radius(2.0),
                            )
                            .child(
                                div()
                                    .id("meeting-hand")
                                    .debug_selector(move || format!("meeting-hand:{account_id}"))
                                    .child(strong(
                                        format!("{name}{}", text::RAISED_A_HAND),
                                        name.len(),
                                    )),
                            )
                            .when(more > 0, |request| {
                                request
                                    .child(div().text_size(u(13.0)).child(text::more_hands(more)))
                            })
                            .child(
                                Button::new("meeting-allow")
                                    .kind(ButtonKind::CallOn)
                                    .label(text::ALLOW)
                                    .tooltip(text::allow_help(&name))
                                    .accessible_label(text::allow_help(&name))
                                    .selector("meeting-allow")
                                    .h(u(28.0))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.state.update(cx, |state, cx| {
                                            state.allow_to_speak(account_id, cx);
                                        });
                                    })),
                            )
                            .into_any_element(),
                    )
                }
                (None, Some(name)) => {
                    let name = name
                        .clone()
                        .unwrap_or_else(|| text::UNKNOWN_PERSON.to_owned());
                    Some(
                        request
                            .id("meeting-allowed")
                            .debug_selector(|| "meeting-allowed".to_owned())
                            .child(strong(
                                format!("{name}{}", text::MAY_SPEAK_NOT_OPEN),
                                name.len(),
                            ))
                            .into_any_element(),
                    )
                }
                (None, None) => None,
            }
        } else if meeting.may_speak {
            Some(
                request
                    .child(icon(Icon::MicrophoneOff, 16.0, colors.call_text))
                    .child(strong(
                        text::YOU_MAY_SPEAK.to_owned(),
                        text::YOU_MAY_SPEAK.len(),
                    ))
                    .child(
                        Button::new("may-speak-unmute")
                            .kind(ButtonKind::CallOn)
                            .icon(Icon::Microphone)
                            .label(text::UNMUTE)
                            .tooltip(text::UNMUTE_HELP)
                            .accessible_label(text::UNMUTE_HELP)
                            .selector("may-speak-unmute")
                            .h(u(28.0))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.state.update(cx, |state, _| state.open_microphone());
                            })),
                    )
                    .into_any_element(),
            )
        } else {
            None
        };
        div()
            .id("meeting-bar")
            .role(Role::Status)
            .aria_label(line.clone())
            .debug_selector(|| "meeting-bar".to_owned())
            .flex()
            .flex_none()
            .items_center()
            .gap(u(10.0))
            .h(u(40.0))
            .pl(u(16.0))
            .pr(u(10.0))
            .overflow_hidden()
            .bg(Hsla::from(colors.cypress))
            .typeface(theme.types.control, theme)
            .font_weight(gpui::FontWeight::MEDIUM)
            .ink(colors.call_text)
            .whitespace_nowrap()
            .child(icon(Icon::Hand, 17.0, colors.call_text))
            .child(
                div()
                    .flex_none()
                    .child(strong(line, text::MEETING_IS_ON.len())),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(u(13.0))
                    .child(if meeting.by_self {
                        text::MEETING_PRESENTER_HELP
                    } else {
                        text::MEETING_PARTICIPANT_HELP
                    }),
            )
            .children(request)
            .into_any_element()
    }

    /// The settings of the room in the Settings tab of a call: meeting mode, the lock, the
    /// user limit, and the setting that hides who is inside.
    pub(crate) fn render_room_settings(
        &mut self,
        room: &RoomView,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let colors = theme.colors;
        let (meeting_on, meeting_held, _) = meeting_switch(room);
        let locked = room.locked;
        let hides = room.hides_occupants;
        let manage = room.allowed.manage;
        let (limit, inside) = (room.user_limit, room.inside);
        let step = |up: bool| stepped_limit(limit, inside, up).filter(|_| manage);
        let lower = step(false).map(|next| Self::limit_listener(next, cx));
        let raise = step(true).map(|next| Self::limit_listener(next, cx));
        let mut rows = vec![
            SectionLabel::new(text::room_section(&room.name)).into_any_element(),
            option_row(
                text::MEETING_MODE,
                text::MEETING_MODE_HELP,
                Toggle::new(
                    "room-meeting",
                    text::MEETING_MODE,
                    meeting_on,
                    (text::ON, text::OFF),
                )
                .held(meeting_held)
                .selector("room-meeting")
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.state
                        .update(cx, |state, cx| state.set_meeting(!meeting_on, cx));
                }))
                .into_any_element(),
                theme,
            )
            .into_any_element(),
            option_row(
                text::LOCK_ROOM,
                text::LOCK_ROOM_SETTING_HELP,
                Toggle::new("room-lock", text::LOCK_ROOM, locked, (text::ON, text::OFF))
                    .held((!room.allowed.lock).then_some(text::LOCK_NO_PERMISSION))
                    .selector("room-lock")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.state
                            .update(cx, |state, cx| state.set_locked(!locked, cx));
                    }))
                    .into_any_element(),
                theme,
            )
            .into_any_element(),
            option_row(
                text::USER_LIMIT,
                text::USER_LIMIT_HELP,
                Stepper::new(
                    "room-limit",
                    text::USER_LIMIT,
                    limit.map_or_else(|| text::NO_USER_LIMIT.to_owned(), |limit| limit.to_string()),
                    text::LOWER_USER_LIMIT,
                    text::RAISE_USER_LIMIT,
                )
                .selector("room-limit")
                .held((!manage).then_some(text::MANAGE_NO_PERMISSION))
                .on_lower(lower)
                .on_raise(raise)
                .into_any_element(),
                theme,
            )
            .into_any_element(),
            option_row(
                text::HIDE_OCCUPANTS,
                text::HIDE_OCCUPANTS_HELP,
                Toggle::new(
                    "room-hide",
                    text::HIDE_OCCUPANTS,
                    hides,
                    (text::ON, text::OFF),
                )
                .held((!manage).then_some(text::MANAGE_NO_PERMISSION))
                .selector("room-hide")
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.state.update(cx, |state, cx| {
                        state.update_room(
                            UpdateCallSettings {
                                user_limit: None,
                                hides_occupants: Some(!hides),
                            },
                            cx,
                        );
                    });
                }))
                .into_any_element(),
                theme,
            )
            .into_any_element(),
        ];
        if !manage {
            rows.push(
                div()
                    .id("room-no-manage")
                    .debug_selector(|| "room-no-manage".to_owned())
                    .flex_none()
                    .px(u(14.0))
                    .pb(u(4.0))
                    .typeface(theme.types.small, theme)
                    .line_height(u(17.0))
                    .ink(colors.ink_2)
                    .child(text::MANAGE_NO_PERMISSION)
                    .into_any_element(),
            );
        }
        rows
    }

    /// The click that sets the user limit of the room of the call.
    fn limit_listener(
        next: Option<u32>,
        cx: &mut Context<Self>,
    ) -> impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static {
        cx.listener(move |this, _, _, cx| {
            this.state.update(cx, |state, cx| {
                state.update_room(
                    UpdateCallSettings {
                        user_limit: Some(next),
                        hides_occupants: None,
                    },
                    cx,
                );
            });
        })
    }

    pub(crate) fn toggle_member_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // A click on the button of the open list closes the list: the press closed it,
        // and the click does not open it again.
        if std::mem::take(&mut self.invite_closed_by_press) {
            return;
        }
        self.invite_open = !self.invite_open;
        if self.invite_open {
            self.invite_find
                .update(cx, |find, cx| find.set_value("", window, cx));
            self.focus_field(&self.invite_find.clone(), window, cx);
        }
        cx.notify();
    }

    /// True while the list of the members who can be invited is open.
    pub fn member_picker_is_open(&self) -> bool {
        self.invite_open
    }

    /// The part under the member list of a call that invites a member (ADR 0514): the
    /// button, what an invitation does, and the list of the members outside the call.
    pub(crate) fn render_invite_area(
        &mut self,
        panel: &CallPanel,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = theme.colors;
        let picker = self
            .invite_open
            .then(|| self.render_member_picker(panel, theme, cx));
        div()
            .id("invite-area")
            .relative()
            .flex_none()
            .pt(u(10.0))
            .px(u(12.0))
            .pb(u(12.0))
            .border_t_1()
            .border_color(Hsla::from(colors.rule))
            .bg(Hsla::from(colors.limestone))
            .children(picker)
            .child(
                Button::new("invite-open")
                    .kind(ButtonKind::Primary)
                    .icon(Icon::Invite)
                    .label(text::INVITE_A_MEMBER)
                    .selector("invite-open")
                    .w_full()
                    .h(u(32.0))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.toggle_member_picker(window, cx);
                    })),
            )
            .child(
                div()
                    .mt(u(7.0))
                    .typeface(theme.types.small, theme)
                    .line_height(u(17.0))
                    .ink(colors.ink_2)
                    .child(text::invite_help(&panel.room.name)),
            )
            .into_any_element()
    }

    fn render_member_picker(
        &mut self,
        panel: &CallPanel,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = theme.colors;
        let wanted = self.invite_find.read(cx).value().trim().to_lowercase();
        let listed: Vec<&OutsideMember> = panel
            .outside
            .iter()
            .filter(|member| wanted.is_empty() || member.name.to_lowercase().contains(&wanted))
            .collect();
        let rows: Vec<AnyElement> = listed
            .iter()
            .enumerate()
            .map(|(index, member)| {
                let account_id = member.account_id;
                let action: AnyElement = match member.invitation {
                    Some(invitation_id) => div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(u(2.0))
                        .child(
                            Button::new(("invited", index))
                                .label(text::INVITED)
                                .disabled(true)
                                .h(u(26.0)),
                        )
                        .child(
                            div()
                                .debug_selector(move || format!("withdraw-invitation:{account_id}"))
                                .child(
                                    Button::new(("withdraw-invitation", index))
                                        .kind(ButtonKind::Ghost)
                                        .icon(Icon::X)
                                        .tooltip(text::WITHDRAW_INVITATION)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.state.update(cx, |state, cx| {
                                                state.withdraw_invitation(invitation_id, cx);
                                            });
                                        })),
                                ),
                        )
                        .into_any_element(),
                    None => div()
                        .flex_none()
                        .debug_selector(move || format!("invite:{account_id}"))
                        .child(
                            Button::new(("invite", index))
                                .label(text::INVITE)
                                .h(u(26.0))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.state
                                        .update(cx, |state, cx| state.invite(account_id, cx));
                                })),
                        )
                        .into_any_element(),
                };
                div()
                    .id(("invite-row", index))
                    .role(Role::ListItem)
                    .aria_label(member.name.clone())
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(u(9.0))
                    .py(u(5.0))
                    .px(u(10.0))
                    .when(index > 0, |row| {
                        row.border_t_1().border_color(Hsla::from(colors.rule_soft))
                    })
                    .child(
                        Mosaic::new(MosaicKind::Large, account_id.as_uuid().as_u128(), 26.0)
                            .radius(2.0),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .truncate()
                                    .typeface(theme.types.control, theme)
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .line_height(u(18.0))
                                    .ink(colors.ink)
                                    .child(member.name.clone()),
                            )
                            .when_some(member.place.as_ref(), |column, place| {
                                column.child(
                                    div()
                                        .truncate()
                                        .typeface(theme.types.small, theme)
                                        .line_height(u(16.0))
                                        .ink(colors.ink_2)
                                        .child(text::in_channel(place)),
                                )
                            }),
                    )
                    .child(action)
                    .into_any_element()
            })
            .collect();
        let empty = rows.is_empty().then(|| {
            div()
                .px(u(10.0))
                .pb(u(6.0))
                .typeface(theme.types.small, theme)
                .line_height(u(17.0))
                .ink(colors.ink_2)
                .child(if panel.outside.is_empty() {
                    text::NO_MEMBER_TO_INVITE
                } else {
                    text::NO_MEMBER_FOUND
                })
        });
        div()
            .absolute()
            .left(u(12.0))
            .right(u(12.0))
            .bottom(u(90.0))
            .child(
                div()
                    .id("member-picker")
                    .role(Role::Dialog)
                    .aria_label(text::invite_to(&panel.room.name))
                    .debug_selector(|| "member-picker".to_owned())
                    .occlude()
                    .pt(u(8.0))
                    .pb(u(4.0))
                    .rounded(u(theme.metrics.radius_menu))
                    .border_1()
                    .border_color(Hsla::from(colors.rule))
                    .bg(Hsla::from(colors.travertine_hi))
                    .shadow(float_shadow(colors.shadow, 8.0, 22.0))
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.invite_open = false;
                        this.invite_closed_by_press = true;
                        cx.notify();
                    }))
                    .child(
                        div()
                            .px(u(10.0))
                            .pb(u(7.0))
                            .typeface(theme.types.control, theme)
                            .text_size(u(13.5))
                            .font_weight(gpui::FontWeight::BOLD)
                            .ink(colors.ink)
                            .child(text::invite_to(&panel.room.name)),
                    )
                    .child(
                        div().mx(u(10.0)).mb(u(6.0)).child(
                            Field::new(&self.invite_find, text::FIND_A_MEMBER)
                                .kind(FieldKind::Find)
                                .selector("invite-find"),
                        ),
                    )
                    .child(
                        div()
                            .id("member-picker-list")
                            .role(Role::List)
                            .aria_label(text::invite_to(&panel.room.name))
                            .flex()
                            .flex_col()
                            .max_h(u(232.0))
                            .overflow_y_scroll()
                            .children(rows)
                            .children(empty),
                    )
                    .child(
                        div()
                            .pt(u(6.0))
                            .px(u(10.0))
                            .pb(u(4.0))
                            .border_t_1()
                            .border_color(Hsla::from(colors.rule_soft))
                            .typeface(theme.types.small, theme)
                            .line_height(u(17.0))
                            .ink(colors.ink_2)
                            .child(text::invite_footer(&panel.room.name)),
                    ),
            )
            .into_any_element()
    }

    /// What the channel list shows under a voice channel that the person cannot enter
    /// without help (ADR 0514): for the selected channel the box that asks the people
    /// inside, with the state of the request; for another full channel a line.
    pub(crate) fn render_entry_box(
        &mut self,
        server_id: ServerId,
        channel: &ChannelRow,
        voice: &VoiceState,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let colors = theme.colors;
        if voice.relation != Relation::Outside {
            return None;
        }
        let channel_id = channel.id;
        let boxed = voice.entry.is_some() || (channel.selected && voice.closed.is_some());
        if !boxed {
            return (voice.closed == Some(Closed::Full)).then(|| {
                div()
                    .id(("full-note", channel_id.as_uuid().as_u128() as usize))
                    .debug_selector(move || format!("full-note:{channel_id}"))
                    .flex_none()
                    .pl(u(37.0))
                    .pr(u(10.0))
                    .pb(u(3.0))
                    .typeface(theme.types.small, theme)
                    .line_height(u(17.0))
                    .ink(colors.ink_2)
                    .child(text::FULL_NOTE)
                    .into_any_element()
            });
        }
        let closed = voice.closed.unwrap_or(Closed::Locked);
        let (shown, title, sentence): (Icon, SharedString, &'static str) = match voice.entry {
            None | Some(EntryState::Asking) => (
                match closed {
                    Closed::Locked => Icon::Lock,
                    Closed::Full => Icon::People,
                },
                text::closed_title(&channel.name, closed).into(),
                text::closed_help(closed),
            ),
            Some(EntryState::Waiting) => (
                Icon::Clock,
                text::ENTRY_WAITING_TITLE.into(),
                text::ENTRY_WAITING,
            ),
            Some(EntryState::Denied) => {
                (Icon::X, text::ENTRY_DENIED_TITLE.into(), text::ENTRY_DENIED)
            }
            Some(EntryState::Expired) => (
                Icon::Clock,
                text::ENTRY_EXPIRED_TITLE.into(),
                text::ENTRY_EXPIRED,
            ),
        };
        let ask = |label: &'static str, disabled: bool, cx: &mut Context<Self>| {
            Button::new("ask-to-enter")
                .label(label)
                .tooltip(text::ASK_TO_ENTER_HELP)
                .accessible_label(text::ASK_TO_ENTER)
                .disabled(disabled)
                .selector("ask-to-enter")
                .flex_1()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.state.update(cx, |state, cx| {
                        state.ask_to_enter(server_id, channel_id, cx);
                    });
                }))
        };
        let withdraw = |label: &'static str, cx: &mut Context<Self>| {
            Button::new("withdraw-entry")
                .label(label)
                .selector("withdraw-entry")
                .flex_1()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.state.update(cx, |state, cx| state.withdraw_entry(cx));
                }))
        };
        let actions = div().flex().gap(u(6.0)).mt(u(7.0));
        let actions = match voice.entry {
            None => actions.child(ask(text::ASK_TO_ENTER, false, cx)),
            Some(EntryState::Asking) => actions.child(ask(text::ENTRY_ASKING, true, cx)),
            Some(EntryState::Waiting) => actions.child(withdraw(text::WITHDRAW_REQUEST, cx)),
            Some(EntryState::Denied | EntryState::Expired) => actions
                .child(ask(text::ASK_AGAIN, false, cx))
                .child(withdraw(text::DISMISS, cx)),
        };
        Some(
            div()
                .id("entry-box")
                .role(Role::Group)
                .aria_label(format!("{title}. {sentence}"))
                .debug_selector(|| "entry-box".to_owned())
                .flex_none()
                .mt(u(3.0))
                .mr(u(10.0))
                .mb(u(5.0))
                .ml(u(37.0))
                .pt(u(7.0))
                .px(u(9.0))
                .pb(u(9.0))
                .rounded(u(theme.metrics.radius))
                .border_1()
                .border_color(Hsla::from(colors.rule))
                .bg(Hsla::from(colors.travertine_hi))
                .shadow(ledge(colors.rule))
                .typeface(theme.types.small, theme)
                .line_height(u(17.0))
                .ink(colors.ink_2)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(u(6.0))
                        .mb(u(2.0))
                        .text_size(u(13.5))
                        .line_height(u(18.0))
                        .font_weight(gpui::FontWeight::BOLD)
                        .ink(colors.ink)
                        .child(icon(shown, 13.0, colors.bronze))
                        .child(div().min_w_0().child(title)),
                )
                .child(sentence)
                .child(actions)
                .into_any_element(),
        )
    }

    /// Opens the menu of a person in a voice channel of the workspace that is shown.
    pub fn open_person_menu(
        &mut self,
        channel_id: ChannelId,
        account_id: AccountId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_layer(
            Layer::PersonMenu(PersonMenuOpen {
                channel_id,
                account_id,
                moving: false,
            }),
            window,
            cx,
        );
    }

    /// Shows the voice channels that the person of the open menu can be moved to.
    pub fn show_move_destinations(&mut self, cx: &mut Context<Self>) {
        if let Layer::PersonMenu(open) = &mut self.layer {
            open.moving = true;
            cx.notify();
        }
    }

    /// The person whose menu is open.
    pub fn person_menu_of(&self) -> Option<(ChannelId, AccountId)> {
        match &self.layer {
            Layer::PersonMenu(open) => Some((open.channel_id, open.account_id)),
            _ => None,
        }
    }

    /// The menu of a person in a voice channel (view K of the design), drawn at the row
    /// of the person. It holds the keyboard focus while it is open.
    pub(crate) fn render_person_menu(
        &mut self,
        menu: &PersonMenu,
        moving: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = theme(cx);
        let colors = theme.colors;
        let name: SharedString = menu
            .name
            .clone()
            .unwrap_or_else(|| text::UNKNOWN_PERSON.to_owned())
            .into();
        let (account_id, room) = (menu.account_id, (menu.server_id, menu.channel_id));
        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(u(9.0))
            .pt(u(6.0))
            .px(u(12.0))
            .pb(u(8.0))
            .mb(u(4.0))
            .border_b_1()
            .border_color(Hsla::from(colors.rule_soft))
            .child(Mosaic::new(MosaicKind::Large, account_id.as_uuid().as_u128(), 26.0).radius(2.0))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .truncate()
                            .text_size(u(14.5))
                            .line_height(u(18.0))
                            .font_weight(gpui::FontWeight::BOLD)
                            .ink(colors.ink)
                            .child(name.clone()),
                    )
                    .child(
                        div()
                            .truncate()
                            .typeface(theme.types.small, &theme)
                            .line_height(u(16.0))
                            .ink(colors.ink_2)
                            .child(text::in_channel(&menu.place)),
                    ),
            );
        let mut items: Vec<AnyElement> = Vec::new();
        if moving {
            items.push(
                MenuItem::new("menu-back", text::BACK)
                    .icon(Icon::ChevronDown)
                    .selector("menu-back")
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Layer::PersonMenu(open) = &mut this.layer {
                            open.moving = false;
                        }
                        cx.notify();
                    }))
                    .into_any_element(),
            );
            items.push(menu_separator(cx).into_any_element());
            items.push(menu_label(text::MOVE_TO, false, cx).into_any_element());
            for (index, (to, channel)) in menu.destinations.iter().enumerate() {
                let to = *to;
                items.push(
                    div()
                        .flex_none()
                        .debug_selector(move || format!("move-to:{to}"))
                        .child(
                            MenuItem::new(("move-to", index), channel.clone())
                                .icon(Icon::Speaker)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.state.update(cx, |state, cx| {
                                        state.move_person(room, account_id, to, cx);
                                    });
                                    this.close_layer(window, cx);
                                })),
                        )
                        .into_any_element(),
                );
            }
        } else {
            if let Some(channel) = &menu.invite_to {
                items.push(
                    MenuItem::new("menu-invite", text::INVITE_TO_MY_ROOM)
                        .icon(Icon::Invite)
                        .detail(text::invite_prompt(&name, channel))
                        .selector("menu-invite")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.state
                                .update(cx, |state, cx| state.invite(account_id, cx));
                            this.close_layer(window, cx);
                        }))
                        .into_any_element(),
                );
            }
            if !menu.destinations.is_empty() {
                items.push(
                    MenuItem::new("menu-move", text::MOVE_TO)
                        .icon(Icon::Leave)
                        .detail(text::PERMISSION_MOVE)
                        .detail_icon(Icon::Key)
                        .end_icon(Icon::ChevronRight)
                        .selector("menu-move")
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Layer::PersonMenu(open) = &mut this.layer {
                                open.moving = true;
                            }
                            cx.notify();
                        }))
                        .into_any_element(),
                );
            }
            if !items.is_empty() {
                items.push(menu_separator(cx).into_any_element());
            }
            let (volume, focus) =
                self.volume_control(menu.server_id, account_id, menu.volume, window, cx);
            items.push(
                div()
                    .flex()
                    .flex_none()
                    .items_start()
                    .gap(u(9.0))
                    .pt(u(5.0))
                    .px(u(12.0))
                    .line_height(u(19.0))
                    .child(
                        div()
                            .mt(u(2.0))
                            .child(icon(Icon::Speaker, 15.0, colors.ink_2)),
                    )
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(text::your_volume_for(&name)),
                    )
                    .into_any_element(),
            );
            items.push(
                div()
                    .flex()
                    .flex_none()
                    .pt(u(5.0))
                    .pr(u(12.0))
                    .pb(u(6.0))
                    .pl(u(36.0))
                    .typeface(theme.types.small, &theme)
                    .ink(colors.ink_2)
                    .child(
                        VolumeSlider::new("menu-volume", &volume, &focus, text::volume_for(&name))
                            .selector("menu-volume"),
                    )
                    .into_any_element(),
            );
            let moderates = menu.microphone_blocked.is_some()
                || menu.sound_blocked.is_some()
                || menu.disconnect;
            if moderates {
                items.push(menu_separator(cx).into_any_element());
                items.push(menu_label(text::MODERATION, false, cx).into_any_element());
            }
            if let Some(blocked) = menu.microphone_blocked {
                items.push(
                    MenuItem::new(
                        "menu-mute",
                        if blocked {
                            text::REMOVE_MICROPHONE_BLOCK
                        } else {
                            text::MUTE_FOR_EVERYONE
                        },
                    )
                    .icon(if blocked {
                        Icon::Microphone
                    } else {
                        Icon::MicrophoneOff
                    })
                    .detail(text::PERMISSION_MUTE)
                    .detail_icon(Icon::Key)
                    .selector("menu-mute")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.state.update(cx, |state, cx| {
                            state.set_microphone_block(room, account_id, !blocked, cx);
                        });
                        this.close_layer(window, cx);
                    }))
                    .into_any_element(),
                );
            }
            if let Some(blocked) = menu.sound_blocked {
                items.push(
                    MenuItem::new(
                        "menu-block-sound",
                        if blocked {
                            text::REMOVE_SOUND_BLOCK
                        } else {
                            text::BLOCK_SOUND
                        },
                    )
                    .icon(if blocked {
                        Icon::Phones
                    } else {
                        Icon::PhonesOff
                    })
                    .detail(text::PERMISSION_DEAFEN)
                    .detail_icon(Icon::Key)
                    .selector("menu-block-sound")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.state.update(cx, |state, cx| {
                            state.set_sound_block(room, account_id, !blocked, cx);
                        });
                        this.close_layer(window, cx);
                    }))
                    .into_any_element(),
                );
            }
            if menu.disconnect {
                items.push(
                    MenuItem::new("menu-disconnect", text::DISCONNECT)
                        .icon(Icon::X)
                        .detail(text::PERMISSION_DISCONNECT)
                        .detail_icon(Icon::Key)
                        .selector("menu-disconnect")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.state
                                .update(cx, |state, cx| state.disconnect(room, account_id, cx));
                            this.close_layer(window, cx);
                        }))
                        .into_any_element(),
                );
            }
        }
        let footer = div()
            .flex_none()
            .mt(u(4.0))
            .pt(u(6.0))
            .px(u(12.0))
            .pb(u(7.0))
            .border_t_1()
            .border_color(Hsla::from(colors.rule_soft))
            .bg(Hsla::from(colors.plaster))
            .typeface(theme.types.small, &theme)
            .line_height(u(17.0))
            .ink(colors.ink_2)
            .child(text::PERSON_MENU_KEYS);
        div()
            .absolute()
            .left(u(68.0))
            .top(u(24.0))
            .child(deferred(
                anchored().snap_to_window().child(
                    Menu::new("person-menu", name, self.layer_focus())
                        .width(292.0)
                        .selector("person-menu")
                        .on_close(Self::handler(cx, |this, window, cx| {
                            this.close_layer(window, cx);
                        }))
                        .child(header)
                        .children(items)
                        .child(footer),
                ),
            ))
            .into_any_element()
    }

    /// The notices about the rules of a call: an invitation that waits for the person, a
    /// person who asks to enter the call of this device, the statement that the person
    /// may speak when the call is not shown, what the application tells about the call,
    /// and a request that the server did not perform.
    pub(crate) fn render_call_rule_notices(
        &mut self,
        view: &ShellView,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let wrap = |notice: Notice| {
            div()
                .flex_none()
                .px(u(16.0))
                .pt(u(10.0))
                .child(notice)
                .into_any_element()
        };
        let pair =
            |first: Button, second: Button| div().flex().gap(u(6.0)).child(first).child(second);
        let mut notices = Vec::new();
        for (index, invited) in view.invitations.iter().enumerate() {
            let (accepted, declined) = (invited.clone(), invited.clone());
            notices.push(wrap(
                Notice::new(text::invitation(
                    invited.by.as_deref(),
                    invited.channel.as_deref(),
                ))
                .icon(Icon::Invite)
                .selector("call-invitation")
                .action(pair(
                    Button::new(("invitation-join", index))
                        .kind(ButtonKind::Primary)
                        .label(text::JOIN)
                        .selector("invitation-join")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.state
                                .update(cx, |state, cx| state.accept_invitation(&accepted, cx));
                        })),
                    Button::new(("invitation-decline", index))
                        .label(text::DECLINE)
                        .selector("invitation-decline")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.state
                                .update(cx, |state, cx| state.decline_invitation(&declined, cx));
                        })),
                )),
            ));
        }
        for (index, prompt) in view.entry_prompts.iter().enumerate() {
            let request_id = prompt.id;
            notices.push(wrap(
                Notice::new(text::entry_prompt(
                    prompt.name.as_deref(),
                    prompt.channel.as_deref().unwrap_or(text::A_VOICE_CHANNEL),
                ))
                .icon(Icon::Lock)
                .selector("entry-prompt")
                .action(pair(
                    Button::new(("entry-accept", index))
                        .kind(ButtonKind::Primary)
                        .label(text::LET_IN)
                        .selector("entry-accept")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.state
                                .update(cx, |state, cx| state.answer_entry(request_id, true, cx));
                        })),
                    Button::new(("entry-deny", index))
                        .label(text::DENY)
                        .selector("entry-deny")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.state.update(cx, |state, cx| {
                                state.answer_entry(request_id, false, cx);
                            });
                        })),
                )),
            ));
        }
        if view
            .call
            .as_ref()
            .is_some_and(|call| call.may_speak && !call.shown)
        {
            notices.push(wrap(
                Notice::new(format!("{}. {}", text::YOU_MAY_SPEAK, text::UNMUTE_HELP))
                    .icon(Icon::MicrophoneOff)
                    .selector("may-speak-notice")
                    .action(
                        Button::new("may-speak-unmute-notice")
                            .kind(ButtonKind::Primary)
                            .icon(Icon::Microphone)
                            .label(text::UNMUTE)
                            .selector("may-speak-unmute-notice")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.state.update(cx, |state, _| state.open_microphone());
                            })),
                    ),
            ));
        }
        let pending = self.state.read(cx).pending();
        if let Some(told) = pending.call_told.clone() {
            notices.push(wrap(
                Notice::new(told)
                    .icon(Icon::Warning)
                    .selector("call-told")
                    .action(
                        Button::new("dismiss-call-told")
                            .label(text::DISMISS)
                            .selector("dismiss-call-told")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.state
                                    .update(cx, |state, cx| state.dismiss_call_told(cx));
                            })),
                    ),
            ));
        }
        if let Some(reason) = pending.call_refused.clone() {
            notices.push(wrap(
                Notice::error(text::call_refused(&reason))
                    .selector("call-refused")
                    .action(
                        Button::new("dismiss-call-refused")
                            .label(text::DISMISS)
                            .selector("dismiss-call-refused")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.state
                                    .update(cx, |state, cx| state.dismiss_call_refusal(cx));
                            })),
                    ),
            ));
        }
        notices
    }
}

#[cfg(test)]
mod tests {
    use super::stepped_limit;

    #[test]
    fn a_step_of_the_user_limit_stays_in_the_range_of_the_server() {
        // Without a limit: nothing to lower, and the first limit is the people inside.
        assert_eq!(stepped_limit(None, 5, false), None);
        assert_eq!(stepped_limit(None, 5, true), Some(Some(5)));
        assert_eq!(stepped_limit(None, 0, true), Some(Some(1)));
        assert_eq!(stepped_limit(None, 500, true), Some(Some(99)));
        // A limit of one that is lowered is removed.
        assert_eq!(stepped_limit(Some(1), 3, false), Some(None));
        assert_eq!(stepped_limit(Some(8), 3, false), Some(Some(7)));
        assert_eq!(stepped_limit(Some(8), 3, true), Some(Some(9)));
        assert_eq!(stepped_limit(Some(99), 3, true), None);
    }
}
