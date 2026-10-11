//! The call of this device (ADR 0735): the header of its voice channel, the stage with the
//! people, the panel with the chat, the members and the sound settings, and the strip of
//! the person's call above the profile strip.
//!
//! Every function here draws from the derived view and reads no window: the call view can
//! be drawn in a window of its own without a change (ADR 0507).

use gpui::{
    AnyElement, AppContext as _, Context, Entity, FocusHandle, Hsla, InteractiveElement,
    IntoElement, MouseButton, ParentElement, Role, SharedString, StatefulInteractiveElement,
    Styled, StyledText, Subscription, Window, anchored, deferred, div, prelude::FluentBuilder, px,
};
use gpui_base::slider::{SliderEvent, SliderState};
use junto_client_core::protocol::ids::{AccountId, ServerId};
use junto_media::calls::Phase;
use junto_media::permission::MicrophoneAccess;

use super::channels::occupant_name;
use super::{Layer, Shell};
use crate::assets::Icon;
use crate::state::view::{
    CallPanel, CallStrip, ChannelRow, DeviceChoice, INPUT_SEGMENTS, Microphone, MicrophoneLine,
    Occupant, Presence, Relation, ShellView, SoundWarning, VoiceState,
};
use crate::text;
use crate::theme::{Theme, theme, u};
use crate::ui::{
    Button, ButtonKind, Flag, FlagKind, Menu, MenuItem, Meter, Mosaic, MosaicKind, PersonTile,
    ScrollArea, SectionLabel, SelectBox, StyleExt, Tab, TileMark, VolumeSlider, float_shadow, icon,
    tooltip,
};

/// The pane of the panel of a call that is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CallTab {
    #[default]
    Chat,
    Members,
    Settings,
}

/// The kind of a sound device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceKind {
    Input,
    Output,
}

/// The volume control of one person: its value, its keyboard focus, and the subscription
/// that carries a change to the state of the application.
pub(crate) struct VolumeControl {
    state: Entity<SliderState>,
    focus: FocusHandle,
    _changes: Subscription,
}

/// The marks on the plate of the tile of a person, and the words for them.
fn tile_marks(occupant: &Occupant) -> (Vec<TileMark>, String) {
    let mut marks = Vec::new();
    let mut words = vec![occupant_name(occupant).to_string()];
    if occupant.hand_raised {
        marks.push(TileMark::HandRaised);
        words.push(text::HAND_RAISED.to_owned());
    }
    if occupant.sharing {
        marks.push(TileMark::Sharing);
        words.push(text::SHARING_SCREEN.to_owned());
    }
    if occupant.sound_off {
        marks.push(TileMark::SoundOff);
        words.push(text::SOUND_OFF.to_owned());
    }
    match occupant.microphone {
        Microphone::Held(reason) => {
            marks.push(TileMark::MicrophoneHeld);
            words.push(text::microphone_held(reason).to_owned());
        }
        Microphone::Closed => {
            marks.push(TileMark::MicrophoneOff);
            words.push(text::MICROPHONE_CLOSED.to_owned());
        }
        Microphone::Open { speaking } => {
            marks.push(TileMark::Level(occupant.level.unwrap_or(0)));
            words.push(
                if speaking {
                    text::SPEAKING
                } else {
                    text::MICROPHONE_OPEN
                }
                .to_owned(),
            );
        }
    }
    (marks, words.join(", "))
}

/// The lines of the state of a person in the member list of a call.
fn member_states(occupant: &Occupant) -> Vec<(Icon, SharedString)> {
    let mut states: Vec<(Icon, SharedString)> = Vec::new();
    match occupant.presence {
        Presence::Connecting => states.push((Icon::Retry, text::CALL_CONNECTING.into())),
        Presence::Reconnecting => states.push((Icon::Retry, text::CALL_RECONNECTING.into())),
        Presence::Here => {}
    }
    if occupant.sharing {
        states.push((Icon::Screen, text::SHARING_SCREEN.into()));
    }
    states.push(match occupant.microphone {
        Microphone::Held(reason) => (
            Icon::MicrophoneHeld,
            text::closed_by(reason, occupant.held_by.as_deref()).into(),
        ),
        Microphone::Closed => (Icon::MicrophoneOff, text::MUTED.into()),
        Microphone::Open { speaking: true } => (Icon::Microphone, text::SPEAKING.into()),
        Microphone::Open { speaking: false } => (Icon::Microphone, text::MICROPHONE_ON.into()),
    });
    if occupant.sound_off {
        states.push((Icon::PhonesOff, text::SOUND_IS_OFF.into()));
    }
    if occupant.hand_raised {
        states.push((Icon::Hand, text::HAND_RAISED.into()));
    }
    states
}

fn occupants(channel: &ChannelRow) -> &[Occupant] {
    channel
        .voice
        .as_ref()
        .map(|voice| voice.occupants.as_slice())
        .unwrap_or_default()
}

impl Shell {
    /// The volume control of one person of the call. A control is made when its person is
    /// drawn first and follows the stored volume when that changes elsewhere.
    fn volume_control(
        &mut self,
        server_id: ServerId,
        account_id: AccountId,
        percent: u16,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Entity<SliderState>, FocusHandle) {
        let key = (server_id, account_id);
        let control = self.volumes.entry(key).or_insert_with(|| {
            let state = cx.new(|_| VolumeSlider::state(percent));
            let changes = cx.subscribe(&state, move |this, _, event: &SliderEvent, cx| {
                let (value, release) = match event {
                    SliderEvent::Change(value) => (value.end(), false),
                    SliderEvent::Release(value) => (value.end(), true),
                };
                let percent = value.round().clamp(0.0, 200.0) as u16;
                this.state.update(cx, |state, cx| {
                    state.set_volume(server_id, account_id, percent, cx);
                    if release {
                        state.store_volume(server_id, account_id, cx);
                    }
                });
            });
            VolumeControl {
                state,
                focus: cx.focus_handle(),
                _changes: changes,
            }
        });
        let shown = control.state.read(cx).value().end().round() as u16;
        if shown != percent {
            control.state.update(cx, |state, cx| {
                state.set_value(f32::from(percent), window, cx)
            });
        }
        (control.state.clone(), control.focus.clone())
    }

    /// What the header of a voice channel shows after its title, for a channel that this
    /// device is not in: the line about its chat, and the action that enters the call.
    pub(crate) fn render_voice_entry(
        &mut self,
        view: &ShellView,
        channel: &ChannelRow,
        voice: &VoiceState,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let colors = theme.colors;
        let (Some(server), Some(workspace)) = (&view.selected, view.workspace()) else {
            return Vec::new();
        };
        let (server_id, workspace_id, channel_id) = (server.id, workspace.id, channel.id);
        let joining = voice.relation == Relation::Joining;
        vec![
            div()
                .flex_none()
                .w(px(1.0))
                .h(u(20.0))
                .bg(Hsla::from(colors.rule))
                .into_any_element(),
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .typeface(theme.types.control, theme)
                .font_weight(gpui::FontWeight::NORMAL)
                .ink(colors.ink_2)
                .child(text::voice_channel_topic(
                    voice.occupants.len(),
                    voice.hidden,
                ))
                .into_any_element(),
            Button::new("join-call")
                .kind(ButtonKind::Green)
                .icon(Icon::Speaker)
                .label(if joining {
                    text::JOINING_CALL
                } else {
                    text::JOIN_CALL
                })
                .tooltip(text::ENTER_VOICE_CHANNEL)
                .accessible_label(text::JOIN_CALL)
                .disabled(joining)
                .selector("join-call")
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.state.update(cx, |state, cx| {
                        state.join_call(server_id, workspace_id, channel_id, cx);
                    });
                }))
                .into_any_element(),
        ]
    }

    /// What the header of the voice channel of the call shows in place of the room
    /// controls while the call is restored.
    pub(crate) fn render_call_interruption(theme: &Theme) -> Vec<AnyElement> {
        vec![
            div()
                .id("call-reconnecting")
                .role(Role::Status)
                .aria_label(text::CALL_RECONNECTING)
                .debug_selector(|| "call-reconnecting".to_owned())
                .flex_none()
                .child(Flag::new(FlagKind::Warning, text::CALL_RECONNECTING).icon(Icon::Retry))
                .into_any_element(),
            div()
                .min_w_0()
                .truncate()
                .typeface(theme.types.small, theme)
                .text_size(u(13.0))
                .ink(theme.colors.ink_2)
                .child(text::ROOM_CONTROLS_RETURN)
                .into_any_element(),
        ]
    }

    /// The buttons of the header that open the panel of the call in a narrow window.
    pub(crate) fn render_call_panel_buttons(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        [
            (CallTab::Chat, Icon::Chat, text::CHAT, "open-call-chat"),
            (
                CallTab::Members,
                Icon::People,
                text::MEMBERS,
                "open-call-members",
            ),
            (
                CallTab::Settings,
                Icon::Gear,
                text::SETTINGS,
                "open-call-settings",
            ),
        ]
        .into_iter()
        .map(|(tab, shown, label, selector)| {
            Button::new(selector)
                .icon(shown)
                .tooltip(label)
                .selector(selector)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.show_call_tab(tab, cx);
                    this.call_panel_open = true;
                }))
                .into_any_element()
        })
        .collect()
    }

    pub(crate) fn show_call_tab(&mut self, tab: CallTab, cx: &mut Context<Self>) {
        self.call_tab = tab;
        if tab == CallTab::Settings {
            self.state.update(cx, |state, cx| state.refresh_devices(cx));
        }
        cx.notify();
    }

    /// The stage of the call: the people as tiles, or what the call waits for.
    pub(crate) fn render_stage(
        &self,
        channel: &ChannelRow,
        panel: &CallPanel,
        theme: &Theme,
    ) -> AnyElement {
        let colors = theme.colors;
        let people = occupants(channel);
        let waiting = |title: &'static str, sentence: String, selector: &'static str| {
            div()
                .id(selector)
                .role(Role::Status)
                .aria_label(title)
                .debug_selector(move || selector.to_owned())
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .w_full()
                .max_w(u(784.0))
                .h_full()
                .max_h(u(490.0))
                .px(u(24.0))
                .rounded(u(theme.metrics.radius_small))
                .border_1()
                .border_color(Hsla::from(colors.basalt_rule))
                .bg(Hsla::from(colors.basalt_2))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(u(10.0))
                        .typeface(theme.types.state_title, theme)
                        .ink(colors.bone)
                        .child(icon(Icon::Retry, 22.0, colors.ochre))
                        .child(title),
                )
                .child(
                    div()
                        .mt(u(8.0))
                        .max_w(u(620.0))
                        .typeface(theme.types.notice, theme)
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .ink(colors.bone_2)
                        .text_center()
                        .child(sentence),
                )
        };
        let viewport = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .items_center()
            .justify_center()
            .p(u(12.0));
        let note = |sentence: &'static str| {
            div()
                .typeface(theme.types.small, theme)
                .text_size(u(13.0))
                .line_height(u(18.0))
                .font_weight(gpui::FontWeight::MEDIUM)
                .ink(colors.bone_2)
                .child(sentence)
        };
        let body = match panel.phase {
            Phase::Connecting => viewport.child(waiting(
                text::CONNECTING_TO_CALL,
                text::CONNECTING_TO_CALL_HELP.to_owned(),
                "call-connecting",
            )),
            Phase::Reconnecting => viewport.child(waiting(
                text::CALL_RECONNECTING,
                text::connection_dropped(panel.interrupted_at, &channel.name),
                "call-lost",
            )),
            Phase::Connected => {
                let tiles = people.iter().enumerate().map(|(index, occupant)| {
                    let (marks, label) = tile_marks(occupant);
                    let name: SharedString = if occupant.is_self {
                        format!("{} ({})", occupant_name(occupant), text::YOU).into()
                    } else {
                        occupant_name(occupant)
                    };
                    PersonTile::new(
                        ("tile", index),
                        name,
                        occupant.account_id.as_uuid().as_u128(),
                        label,
                    )
                    .speaking(matches!(
                        occupant.microphone,
                        Microphone::Open { speaking: true }
                    ))
                    .marks(marks)
                    .selector(format!("call-tile:{}", occupant.account_id))
                });
                viewport
                    .child(
                        div()
                            .id("call-people")
                            .role(Role::List)
                            .aria_label(text::IN_THE_CALL)
                            .debug_selector(|| "call-people".to_owned())
                            .flex()
                            .flex_wrap()
                            .justify_center()
                            .content_center()
                            .gap(u(8.0))
                            .w_full()
                            .children(tiles),
                    )
                    .when(people.len() < 2, |viewport| {
                        viewport.child(div().mt(u(14.0)).child(note(text::ALONE_IN_CALL)))
                    })
            }
        };
        div()
            .id("call-stage")
            .debug_selector(|| "call-stage".to_owned())
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .bg(Hsla::from(colors.stage))
            .child(body)
            .when(panel.phase == Phase::Reconnecting, |stage| {
                stage.child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .h(u(106.0))
                        .px(u(12.0))
                        .pb(u(14.0))
                        .border_t_1()
                        .border_color(Hsla::from(colors.basalt_rule))
                        .bg(Hsla::from(colors.basalt_deep))
                        .child(note(text::SOUND_RETURNS)),
                )
            })
            .into_any_element()
    }

    /// The panel of a call, with the row of its three tabs.
    pub(crate) fn render_call_panel(
        &mut self,
        view: &ShellView,
        channel: &ChannelRow,
        panel: &CallPanel,
        narrow: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = &theme(cx);
        let colors = theme.colors;
        let tab = |tab: CallTab,
                   shown: Icon,
                   label: &'static str,
                   selector: &'static str,
                   cx: &mut Context<Self>| {
            Tab::new(selector, shown, label)
                .on(self.call_tab == tab)
                .selector(selector)
                .on_click(cx.listener(move |this, _, _, cx| this.show_call_tab(tab, cx)))
        };
        let tabs = div()
            .id("call-tabs")
            .role(Role::TabList)
            .aria_label(channel.name.clone())
            .flex()
            .flex_none()
            .items_stretch()
            .h(u(42.0))
            .px(u(8.0))
            .border_b_1()
            .border_color(Hsla::from(colors.rule))
            .bg(Hsla::from(colors.limestone))
            .child(tab(
                CallTab::Chat,
                Icon::Chat,
                text::CHAT,
                "call-tab-chat",
                cx,
            ))
            .child(tab(
                CallTab::Members,
                Icon::People,
                text::MEMBERS,
                "call-tab-members",
                cx,
            ))
            .child(tab(
                CallTab::Settings,
                Icon::Gear,
                text::SETTINGS,
                "call-tab-settings",
                cx,
            ))
            .when(narrow, |tabs| {
                tabs.child(div().flex_1()).child(
                    div().flex().items_center().child(
                        Button::new("close-call-panel")
                            .kind(ButtonKind::Ghost)
                            .icon(Icon::X)
                            .tooltip(text::CLOSE_PANEL)
                            .selector("close-call-panel")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.call_panel_open = false;
                                cx.notify();
                            })),
                    ),
                )
            });
        let pane = match self.call_tab {
            CallTab::Chat => Self::render_call_chat(channel, theme),
            CallTab::Members => self.render_call_members(view, channel, panel, theme, window, cx),
            CallTab::Settings => self.render_call_settings(panel, theme, cx),
        };
        div()
            .id("call-panel")
            .role(Role::Complementary)
            .aria_label(channel.name.clone())
            .debug_selector(|| "call-panel".to_owned())
            .flex()
            .flex_col()
            .flex_none()
            .w(u(theme.metrics.call_panel_width))
            .h_full()
            .bg(Hsla::from(colors.plaster))
            .border_l_1()
            .border_color(Hsla::from(colors.rule))
            .child(tabs)
            .child(pane)
            .into_any_element()
    }

    fn render_call_chat(channel: &ChannelRow, theme: &Theme) -> AnyElement {
        let colors = theme.colors;
        div()
            .id("call-chat")
            .debug_selector(|| "call-chat".to_owned())
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(u(8.0))
                    .h(u(38.0))
                    .pl(u(14.0))
                    .pr(u(10.0))
                    .border_b_1()
                    .border_color(Hsla::from(colors.rule_soft))
                    .typeface(theme.types.small, theme)
                    .text_size(u(13.0))
                    .ink(colors.ink_2)
                    .child(
                        div()
                            .flex_none()
                            .max_w(u(180.0))
                            .truncate()
                            .typeface(theme.types.name, theme)
                            .ink(colors.ink)
                            .child(channel.name.clone()),
                    )
                    .child(text::VOICE_CHANNEL_CHAT),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .justify_end()
                    .px(u(14.0))
                    .pb(u(14.0))
                    .child(
                        div()
                            .typeface(theme.types.heading, theme)
                            .ink(colors.ink)
                            .child(text::MESSAGES_NOT_AVAILABLE_TITLE),
                    )
                    .child(
                        div()
                            .mt(u(3.0))
                            .typeface(theme.types.small, theme)
                            .ink(colors.ink_2)
                            .child(text::VOICE_CHAT_NOT_AVAILABLE),
                    ),
            )
            .into_any_element()
    }

    fn render_call_members(
        &mut self,
        view: &ShellView,
        channel: &ChannelRow,
        panel: &CallPanel,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = theme.colors;
        let server_id = view.selected.as_ref().map(|server| server.id);
        let state_line = |states: Vec<(Icon, SharedString)>| {
            let mut line = div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(u(5.0))
                .typeface(theme.types.small, theme)
                .line_height(u(18.0))
                .ink(colors.ink_2);
            for (index, (shown, label)) in states.into_iter().enumerate() {
                if index > 0 {
                    line = line.child(
                        div()
                            .flex_none()
                            .w(px(1.0))
                            .h(u(10.0))
                            .mx(u(3.0))
                            .bg(Hsla::from(colors.rule)),
                    );
                }
                line = line.child(icon(shown, 13.0, colors.ink_3)).child(label);
            }
            line
        };
        let row = |index: usize, account_id: AccountId, label: SharedString| {
            div()
                .id(("call-member", index))
                .role(Role::ListItem)
                .aria_label(label)
                .debug_selector(move || format!("call-member:{account_id}"))
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
                    account_id.as_uuid().as_u128(),
                    30.0,
                )))
        };
        let name_line = |name: SharedString, is_self: bool| {
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
                        .child(name),
                )
                .when(is_self, |line| {
                    line.child(
                        div()
                            .flex_none()
                            .typeface(theme.types.small, theme)
                            .ink(colors.ink_2)
                            .child(text::YOU),
                    )
                })
        };
        let people = occupants(channel);
        let mut inside: Vec<AnyElement> = Vec::new();
        for (index, occupant) in people.iter().enumerate() {
            let name = occupant_name(occupant);
            let (_, label) = tile_marks(occupant);
            let mark: AnyElement = match occupant.microphone {
                Microphone::Held(_) => {
                    icon(Icon::MicrophoneHeld, 14.0, colors.terracotta_deep).into_any_element()
                }
                Microphone::Closed => {
                    icon(Icon::MicrophoneOff, 14.0, colors.terracotta_deep).into_any_element()
                }
                Microphone::Open { .. } => {
                    Meter::new(occupant.level.unwrap_or(0)).into_any_element()
                }
            };
            let volume = match (occupant.is_self, server_id) {
                (false, Some(server_id)) => {
                    let (state, focus) = self.volume_control(
                        server_id,
                        occupant.account_id,
                        occupant.volume,
                        window,
                        cx,
                    );
                    Some(
                        div()
                            .flex()
                            .items_center()
                            .gap(u(8.0))
                            .mt(u(3.0))
                            .typeface(theme.types.small, theme)
                            .ink(colors.ink_2)
                            .child(text::VOLUME)
                            .child(
                                VolumeSlider::new(
                                    ("volume", index),
                                    &state,
                                    &focus,
                                    text::volume_for(&name),
                                )
                                .selector(format!("volume:{}", occupant.account_id)),
                            ),
                    )
                }
                _ => None,
            };
            inside.push(
                row(index, occupant.account_id, label.into())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                name_line(name, occupant.is_self)
                                    .child(div().flex_1())
                                    .child(div().flex().flex_none().items_center().child(mark)),
                            )
                            .child(state_line(member_states(occupant)))
                            .children(volume),
                    )
                    .into_any_element(),
            );
        }
        let outside = panel.outside.iter().enumerate().map(|(index, member)| {
            row(
                people.len() + index,
                member.account_id,
                member.name.clone().into(),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(name_line(member.name.clone().into(), false))
                    .when_some(member.place.as_ref(), |column, place| {
                        column.child(state_line(vec![(
                            Icon::Speaker,
                            text::in_channel(place).into(),
                        )]))
                    }),
            )
            .into_any_element()
        });
        let outside: Vec<AnyElement> = outside.collect();
        ScrollArea::new("call-members", &self.call_members_scroll, text::MEMBERS)
            .selector("call-members")
            .child(SectionLabel::new(text::IN_THE_CALL).count(people.len()))
            .children(inside)
            .when(!outside.is_empty(), |list| {
                list.child(SectionLabel::new(text::NOT_IN_THE_CALL).count(outside.len()))
                    .children(outside)
            })
            .child(div().flex_none().h(u(4.0)))
            .into_any_element()
    }

    fn render_call_settings(
        &mut self,
        panel: &CallPanel,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = theme.colors;
        let audio = &panel.audio;
        let shown = |choices: &[DeviceChoice]| -> SharedString {
            let chosen = choices.iter().find(|choice| choice.selected);
            match chosen {
                Some(choice) if choice.id.is_none() => text::system_default(&choice.name).into(),
                Some(choice) if choice.name.is_empty() => text::DEVICE_NOT_CONNECTED.into(),
                Some(choice) => choice.name.clone().into(),
                None => text::SYSTEM_DEFAULT.into(),
            }
        };
        let open = match &self.layer {
            Layer::DeviceMenu(kind) => Some(*kind),
            _ => None,
        };
        let field = |kind: DeviceKind,
                     label: &'static str,
                     choices: &[DeviceChoice],
                     selector: &'static str,
                     this: &mut Self,
                     cx: &mut Context<Self>| {
            let menu = (open == Some(kind)).then(|| this.render_device_menu(kind, choices, cx));
            div()
                .flex_none()
                .pt(u(4.0))
                .pb(u(5.0))
                .px(u(14.0))
                .child(
                    div()
                        .mb(u(3.0))
                        .typeface(theme.types.control, theme)
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .ink(colors.ink)
                        .child(label),
                )
                .child(
                    div()
                        .relative()
                        .child(
                            SelectBox::new(selector, label, shown(choices))
                                .selector(selector)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.open_device_menu(kind, window, cx);
                                })),
                        )
                        .children(menu),
                )
        };
        let input = field(
            DeviceKind::Input,
            text::INPUT_DEVICE,
            &audio.inputs,
            "input-device",
            self,
            cx,
        );
        let output = field(
            DeviceKind::Output,
            text::OUTPUT_DEVICE,
            &audio.outputs,
            "output-device",
            self,
            cx,
        );
        let small = |sentence: SharedString| {
            div()
                .typeface(theme.types.small, theme)
                .ink(colors.ink_2)
                .child(sentence)
        };
        let unusable = matches!(
            audio.microphone,
            MicrophoneLine::NoPermission | MicrophoneLine::Failed
        ) || audio.access == MicrophoneAccess::Refused;
        ScrollArea::new("call-settings", &self.call_settings_scroll, text::SETTINGS)
            .selector("call-settings")
            .child(SectionLabel::new(text::YOUR_AUDIO))
            .child(
                input.child(
                    div()
                        .id("input-level")
                        .role(Role::Meter)
                        .aria_label(text::INPUT_LEVEL)
                        .aria_numeric_value(audio.input_level as f64)
                        .aria_max_numeric_value(INPUT_SEGMENTS as f64)
                        .debug_selector(|| "input-level".to_owned())
                        .flex()
                        .items_center()
                        .gap(u(9.0))
                        .mt(u(6.0))
                        .typeface(theme.types.small, theme)
                        .ink(colors.ink_2)
                        .child(text::INPUT_LEVEL)
                        .child(Meter::wide(audio.input_level)),
                ),
            )
            .child(output)
            .child(
                div()
                    .id("microphone-access")
                    .role(Role::Status)
                    .aria_label(text::microphone_access(audio.access))
                    .debug_selector(|| "microphone-access".to_owned())
                    .flex()
                    .flex_col()
                    .flex_none()
                    .gap(u(6.0))
                    .pt(u(6.0))
                    .px(u(14.0))
                    .child(small(text::microphone_access(audio.access).into()))
                    .when(unusable, |column| {
                        column.child(
                            div().flex().child(
                                Button::new("retry-microphone")
                                    .label(text::TRY_AGAIN)
                                    .icon(Icon::Retry)
                                    .selector("retry-microphone")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.state
                                            .update(cx, |state, cx| state.retry_microphone(cx));
                                    })),
                            ),
                        )
                    })
                    .child(small(text::MORE_SOUND_SETTINGS.into())),
            )
            .child(div().flex_none().h(u(8.0)))
            .into_any_element()
    }

    pub(crate) fn open_device_menu(
        &mut self,
        kind: DeviceKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A click on the box of an open list closes the list: the press closed it, and
        // the click must not open it again.
        if std::mem::take(&mut self.device_menu_closed_by_press) {
            return;
        }
        self.state.update(cx, |state, cx| state.refresh_devices(cx));
        self.open_layer(Layer::DeviceMenu(kind), window, cx);
    }

    /// The list of the devices of one kind, under its box. It holds the keyboard focus
    /// while it is open.
    fn render_device_menu(
        &mut self,
        kind: DeviceKind,
        choices: &[DeviceChoice],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = theme(cx);
        let label = match kind {
            DeviceKind::Input => text::INPUT_DEVICE,
            DeviceKind::Output => text::OUTPUT_DEVICE,
        };
        let items: Vec<AnyElement> = choices
            .iter()
            .enumerate()
            .map(|(index, choice)| {
                let id = choice.id.clone();
                let name: SharedString = match (&choice.id, choice.name.is_empty()) {
                    (None, _) => text::system_default(&choice.name).into(),
                    (Some(_), true) => text::DEVICE_NOT_CONNECTED.into(),
                    (Some(_), false) => choice.name.clone().into(),
                };
                div()
                    .flex_none()
                    .debug_selector(move || format!("device-choice-{index}"))
                    .child(
                        MenuItem::new(("device", index), name)
                            .icon(if choice.selected {
                                Icon::Check
                            } else {
                                match kind {
                                    DeviceKind::Input => Icon::Microphone,
                                    DeviceKind::Output => Icon::Phones,
                                }
                            })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                let id = id.clone();
                                this.state.update(cx, |state, cx| match kind {
                                    DeviceKind::Input => state.choose_input(id, cx),
                                    DeviceKind::Output => state.choose_output(id, cx),
                                });
                                this.close_layer(window, cx);
                            })),
                    )
                    .into_any_element()
            })
            .collect();
        div()
            .absolute()
            .left_0()
            .top(u(34.0))
            .child(deferred(
                anchored().snap_to_window().child(
                    Menu::new("device-menu", label, self.layer_focus())
                        .width(theme.metrics.call_panel_width - 28.0)
                        .selector("device-menu")
                        .on_close(Self::handler(cx, |this, window, cx| {
                            this.device_menu_closed_by_press = !window.last_input_was_keyboard();
                            this.close_layer(window, cx);
                        }))
                        .children(items)
                        .child(div().h(u(4.0))),
                ),
            ))
            .into_any_element()
    }

    /// The strip of the person's call, above the profile strip. It is drawn while this
    /// device is in a call, whatever server and channel the window shows.
    pub(crate) fn render_call_strip(
        &mut self,
        strip: &CallStrip,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = theme.colors;
        let name: SharedString = strip
            .name
            .clone()
            .unwrap_or_else(|| text::A_VOICE_CHANNEL.to_owned())
            .into();
        let interrupted = strip.phase == Phase::Reconnecting;
        let (ground, ink) = if interrupted {
            (colors.ochre, colors.ink)
        } else {
            (colors.cypress, colors.call_text)
        };
        let leave = |cx: &mut Context<Self>| {
            Button::new("leave-call")
                .kind(ButtonKind::CallLeave)
                .icon(Icon::Leave)
                .label(text::LEAVE)
                .tooltip(text::LEAVE_CALL)
                .accessible_label(text::LEAVE_CALL)
                .selector("leave-call")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.state.update(cx, |state, cx| state.leave_call(cx));
                }))
        };
        let state_word = match strip.phase {
            Phase::Connected => text::IN_CALL,
            Phase::Connecting => text::CALL_CONNECTING,
            Phase::Reconnecting => text::CALL_RECONNECTING,
        };
        // The state in strong type and the name of the voice channel, as one line of text
        // that is cut at the width of the strip.
        let line = format!("{state_word} · {name}");
        let strong = if interrupted {
            0..state_word.len()
        } else {
            line.len() - name.len()..line.len()
        };
        let label = line.clone();
        let first = div()
            .flex()
            .items_center()
            .gap(u(7.0))
            .typeface(theme.types.control, theme)
            .text_size(u(13.5))
            .line_height(u(18.0))
            .child(icon(
                if interrupted {
                    Icon::Retry
                } else {
                    Icon::Speaker
                },
                15.0,
                ink,
            ))
            .child(
                div()
                    .id("show-call")
                    .role(Role::Button)
                    .aria_label(label)
                    .debug_selector(|| "show-call".to_owned())
                    .tooltip(tooltip(text::SHOW_CALL))
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .cursor_pointer()
                    .focusable()
                    .tab_stop(true)
                    .focus_visible(move |style| {
                        style.shadow(crate::ui::focus_ring(colors.ochre, 2.0))
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.state.update(cx, |state, cx| state.show_call(cx));
                    }))
                    .child(StyledText::new(line).with_highlights([(
                        strong,
                        gpui::HighlightStyle {
                            font_weight: Some(gpui::FontWeight::BOLD),
                            ..gpui::HighlightStyle::default()
                        },
                    )])),
            )
            .when_some(
                strip.elapsed.filter(|_| strip.phase == Phase::Connected),
                |line, elapsed| {
                    line.child(
                        div()
                            .id("call-time")
                            .debug_selector(|| "call-time".to_owned())
                            .tooltip(tooltip(text::CALL_TIME))
                            .flex_none()
                            .typeface(theme.types.mono_field, theme)
                            .text_size(u(13.5))
                            .ink(colors.call_text_hi)
                            .child(text::call_time(elapsed)),
                    )
                },
            );
        let second = if interrupted {
            div()
                .flex()
                .items_center()
                .gap(u(8.0))
                .mt(u(7.0))
                .typeface(theme.types.small, theme)
                .line_height(u(16.0))
                .font_weight(gpui::FontWeight::MEDIUM)
                .child(div().flex_1().min_w_0().child(text::MICROPHONE_PAUSED))
                .child(leave(cx))
        } else {
            // The buttons of functions that this version does not have are drawn, so that
            // the strip has its final form (ADR 0735).
            let later = |id: &'static str, shown: Icon, why: &'static str| {
                Button::new(id)
                    .kind(ButtonKind::Call)
                    .icon(shown)
                    .tooltip(why)
                    .disabled(true)
            };
            div()
                .flex()
                .items_center()
                .gap(u(5.0))
                .mt(u(7.0))
                .child(later(
                    "call-camera",
                    Icon::Camera,
                    text::CAMERA_NOT_AVAILABLE,
                ))
                .child(later("call-share", Icon::Screen, text::SHARE_NOT_AVAILABLE))
                .child(later("call-hand", Icon::Hand, text::HAND_NOT_AVAILABLE))
                .child(div().flex_1())
                .child(leave(cx))
        };
        div()
            .id("call-strip")
            .role(Role::Group)
            .aria_label(text::IN_CALL)
            .debug_selector(|| "call-strip".to_owned())
            .flex_none()
            .pt(u(8.0))
            .pb(u(10.0))
            .pl(u(12.0))
            .pr(u(10.0))
            .bg(Hsla::from(ground))
            .ink(ink)
            .child(first)
            .child(second)
            .into_any_element()
    }

    /// The line above the profile strip about the sound of this device.
    pub(crate) fn render_sound_warning(
        &mut self,
        warning: &SoundWarning,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = theme.colors;
        let (strong, rest) = text::sound_warning(warning);
        let label = format!("{strong}{rest}");
        let retry = matches!(
            warning,
            SoundWarning::NoMicrophoneAccess | SoundWarning::MicrophoneFailed
        );
        let shown = match warning {
            SoundWarning::MicrophoneHeld { .. } => Icon::MicrophoneHeld,
            SoundWarning::NoMicrophoneAccess | SoundWarning::MicrophoneFailed => {
                Icon::MicrophoneOff
            }
            SoundWarning::SoundHeld { .. } | SoundWarning::OutputFailed => Icon::PhonesOff,
        };
        div()
            .id("sound-warning")
            .role(Role::Status)
            .aria_label(label.clone())
            .debug_selector(|| "sound-warning".to_owned())
            .flex()
            .flex_none()
            .items_start()
            .gap(u(8.0))
            .pt(u(7.0))
            .pb(u(8.0))
            .pl(u(12.0))
            .pr(u(10.0))
            .bg(Hsla::from(colors.terracotta_wash))
            .border_t_1()
            .border_color(Hsla::from(colors.terracotta_wash_rule))
            .typeface(theme.types.small, theme)
            .text_size(u(13.0))
            .line_height(u(18.0))
            .ink(colors.ink)
            .child(
                div()
                    .mt(u(1.0))
                    .child(icon(shown, 15.0, colors.terracotta_deep)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(StyledText::new(label).with_highlights([(
                        0..strong.len(),
                        gpui::HighlightStyle {
                            font_weight: Some(gpui::FontWeight::BOLD),
                            ..gpui::HighlightStyle::default()
                        },
                    )]))
                    .when(retry, |column| {
                        column.child(
                            div().flex().mt(u(6.0)).child(
                                Button::new("warning-retry-microphone")
                                    .label(text::TRY_AGAIN)
                                    .icon(Icon::Retry)
                                    .selector("warning-retry-microphone")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.state
                                            .update(cx, |state, cx| state.retry_microphone(cx));
                                    })),
                            ),
                        )
                    }),
            )
            .into_any_element()
    }

    /// The call in the main area: the stage, and the panel beside it or over it.
    pub(crate) fn render_call(
        &mut self,
        view: &ShellView,
        channel: &ChannelRow,
        panel: &CallPanel,
        narrow: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (AnyElement, Option<AnyElement>) {
        let stage = self.render_stage(channel, panel, &theme(cx));
        let side = (!narrow || self.call_panel_open)
            .then(|| self.render_call_panel(view, channel, panel, narrow, window, cx));
        (stage, side)
    }

    /// The cover behind a panel that lies over the content in a narrow window.
    pub(crate) fn side_cover(
        theme: &Theme,
        cx: &mut Context<Self>,
        close: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> AnyElement {
        div()
            .id("side-cover")
            .absolute()
            .inset_0()
            .occlude()
            .bg(Hsla::from(theme.colors.scrim_soft))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    close(this, cx);
                    cx.notify();
                }),
            )
            .into_any_element()
    }

    pub(crate) fn side_overlay(theme: &Theme, side: AnyElement) -> AnyElement {
        div()
            .absolute()
            .top_0()
            .bottom_0()
            .right_0()
            .flex()
            .occlude()
            .shadow(float_shadow(theme.colors.shadow_deep, 0.0, 26.0))
            .child(side)
            .into_any_element()
    }
}
