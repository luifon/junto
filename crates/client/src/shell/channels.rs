//! The channel column: the selected workspace of the selected server with its channels,
//! the people in its voice channels, and the profile strip of the person.

use gpui::{
    AnyElement, Context, Hsla, InteractiveElement, IntoElement, ParentElement, Role, SharedString,
    StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder,
};
use junto_client_core::protocol::structure::ChannelKind;

use super::{Layer, Shell};
use crate::assets::Icon;
use crate::state::view::{
    Content, Microphone, MicrophoneButton, Occupant, Presence, SectionTitle, ServerStatus,
    ShellView, VoiceState, WorkspaceView,
};
use crate::text;
use crate::theme::{Theme, theme, u};
use crate::ui::{
    Band, BandKind, Button, ButtonKind, CategoryHeader, ChannelRow, ChannelState, Mosaic,
    MosaicKind, OccupantMark, OccupantRow, ScrollArea, StyleExt, icon, tooltip,
};

/// The lit segments of the meter of a person who speaks in a call that this device is not
/// in. The server tells who speaks there and no level (ADR 0513).
const SPEAKING_SEGMENTS: usize = 3;

/// The states that the row of a voice channel shows at its right.
pub(crate) fn channel_states(voice: &VoiceState) -> Vec<ChannelState> {
    let mut states = Vec::new();
    if voice.recording {
        states.push(ChannelState::Recording(text::RECORDING_MARK.into()));
    }
    if voice.locked {
        states.push(ChannelState::Marked {
            icon: Icon::Lock,
            text: text::LOCKED.into(),
            strong: false,
        });
    } else if voice.hidden {
        states.push(ChannelState::Marked {
            icon: Icon::EyeOff,
            text: text::IN_USE.into(),
            strong: false,
        });
    } else if let (true, Some(limit)) = (voice.full, voice.user_limit) {
        states.push(ChannelState::Marked {
            icon: Icon::People,
            text: text::occupancy(voice.occupants.len(), limit, true).into(),
            strong: true,
        });
    }
    states
}

/// The marks of a person in a voice channel, in the order of the design: hand, screen,
/// sound, and the microphone last.
pub(crate) fn occupant_marks(occupant: &Occupant) -> Vec<OccupantMark> {
    let mut marks = Vec::new();
    if occupant.hand_raised {
        marks.push(OccupantMark::HandRaised(text::HAND_RAISED.into()));
    }
    if occupant.sharing {
        marks.push(OccupantMark::Sharing(text::SHARING_SCREEN.into()));
    }
    if occupant.sound_off {
        marks.push(OccupantMark::SoundOff(text::SOUND_OFF.into()));
    }
    marks.push(match (occupant.presence, occupant.microphone) {
        (Presence::Connecting, _) => OccupantMark::Status(text::STATUS_CONNECTING.into()),
        (Presence::Reconnecting, _) => OccupantMark::Status(text::STATUS_RECONNECTING.into()),
        (Presence::Here, Microphone::Held(reason)) => {
            OccupantMark::MicrophoneHeld(text::microphone_held(reason).into())
        }
        (Presence::Here, Microphone::Closed) => {
            OccupantMark::MicrophoneOff(text::MICROPHONE_CLOSED.into())
        }
        (Presence::Here, Microphone::Open { speaking }) => OccupantMark::Level {
            // In the call of this device the media engine measures the level.
            lit: occupant
                .level
                .unwrap_or(if speaking { SPEAKING_SEGMENTS } else { 0 }),
            label: if speaking {
                text::SPEAKING.into()
            } else {
                text::MICROPHONE_OPEN.into()
            },
        },
    });
    marks
}

pub(crate) fn occupant_name(occupant: &Occupant) -> SharedString {
    occupant
        .name
        .clone()
        .unwrap_or_else(|| text::UNKNOWN_PERSON.to_owned())
        .into()
}

impl Shell {
    pub(crate) fn render_channel_column(
        &mut self,
        view: &ShellView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = theme(cx);
        let colors = theme.colors;
        let workspace = view.workspace();
        let (overline, title): (Option<SharedString>, SharedString) =
            match (&view.selected, workspace) {
                (Some(server), Some(workspace)) => (
                    Some(server.name.clone().into()),
                    workspace.name.clone().into(),
                ),
                (Some(server), None) => (None, server.name.clone().into()),
                (None, _) => (None, SharedString::default()),
            };
        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .h(u(theme.metrics.header_height))
            .pl(u(14.0))
            .pr(u(12.0))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .when_some(overline, |column, overline| {
                        column.child(
                            div()
                                .typeface(theme.types.overline, &theme)
                                .ink(colors.bronze)
                                .truncate()
                                .child(overline),
                        )
                    })
                    .child(
                        div()
                            .id("workspace-title")
                            .role(Role::Heading)
                            .aria_level(1)
                            .aria_label(title.clone())
                            .debug_selector(|| "workspace-title".to_owned())
                            .typeface(theme.types.title, &theme)
                            .ink(colors.ink)
                            .truncate()
                            .child(title),
                    ),
            )
            .when(view.selected.is_some(), |header| {
                header.child(
                    Button::new("server-menu")
                        .kind(ButtonKind::Ghost)
                        .icon(Icon::ChevronDown)
                        .tooltip(text::SERVER_MENU)
                        .selector("server-menu-button")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_server_menu(window, cx);
                        })),
                )
            });

        let list: AnyElement = match (&view.selected, workspace) {
            (Some(_), Some(workspace)) => self
                .render_channel_list(view, workspace, &theme, window, cx)
                .into_any_element(),
            (Some(server), None) if matches!(server.content, Content::NoWorkspace) => div()
                .flex_1()
                .px(u(14.0))
                .pt(u(12.0))
                .typeface(theme.types.small, &theme)
                .ink(colors.ink_2)
                .child(text::NO_WORKSPACES)
                .into_any_element(),
            _ => div().flex_1().into_any_element(),
        };
        let stale = view
            .selected
            .as_ref()
            .is_some_and(|server| server.status == ServerStatus::Reconnecting);

        div()
            .id("channel-column")
            .relative()
            .flex()
            .flex_col()
            .flex_none()
            .w(u(theme.metrics.channel_column_width))
            .h_full()
            .bg(Hsla::from(colors.limestone))
            .border_r_1()
            .border_color(Hsla::from(colors.rule))
            .child(header)
            .child(Band::new(BandKind::Tessera))
            .child(list)
            .when(stale, |column| {
                column.child(
                    div()
                        .id("stale-state")
                        .role(Role::Status)
                        .aria_label(text::STATE_STALE)
                        .debug_selector(|| "stale-state".to_owned())
                        .tooltip(tooltip(text::STATE_STALE))
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(u(7.0))
                        .h(u(30.0))
                        .mx(u(8.0))
                        .mb(u(8.0))
                        .px(u(9.0))
                        .rounded(u(theme.metrics.radius))
                        .bg(Hsla::from(colors.ochre))
                        .typeface(theme.types.control_strong, &theme)
                        .text_size(u(13.0))
                        .ink(colors.ink)
                        .child(icon(Icon::Retry, 14.0, colors.ink))
                        .child(text::STATUS_RECONNECTING),
                )
            })
            .when_some(view.call.as_ref(), |column, strip| {
                column.child(self.render_call_strip(strip, &theme, cx))
            })
            .when_some(view.controls.warning.as_ref(), |column, warning| {
                column.child(self.render_sound_warning(warning, &theme, cx))
            })
            .child(self.render_profile_strip(view, &theme, cx))
    }

    fn render_channel_list(
        &mut self,
        view: &ShellView,
        workspace: &WorkspaceView,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = theme.colors;
        let workspace_id = workspace.id;
        let server_id = view.selected.as_ref().map(|server| server.id);
        // The person whose menu is open, and the menu.
        let open_menu = match &self.layer {
            Layer::PersonMenu(open) => view
                .person_menu(open.channel_id, open.account_id)
                .map(|menu| (menu, open.moving)),
            _ => None,
        };
        let mut rows: Vec<AnyElement> = Vec::new();
        let mut channel_index = 0usize;
        for (position, section) in workspace.sections.iter().enumerate() {
            let title: SharedString = match &section.title {
                SectionTitle::TextChannels => text::TEXT_CHANNELS.into(),
                SectionTitle::VoiceChannels => text::VOICE_CHANNELS.into(),
                SectionTitle::Category(name) => name.clone().into(),
            };
            rows.push(
                CategoryHeader::new(title)
                    .first(position == 0)
                    .into_any_element(),
            );
            let mut after_occupants = false;
            for channel in &section.rows {
                let channel_id = channel.id;
                let index = channel_index;
                channel_index += 1;
                let row = ChannelRow::new(
                    ("channel", index),
                    match channel.kind {
                        ChannelKind::Text => Icon::Hash,
                        ChannelKind::Voice => Icon::Speaker,
                    },
                    channel.name.clone(),
                )
                .selected(channel.selected)
                .unread(unread_sign(channel.unread))
                .muted(channel.unread.muted)
                .states(
                    channel
                        .voice
                        .as_ref()
                        .map(channel_states)
                        .unwrap_or_default(),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.state.update(cx, |state, cx| {
                        state.select_channel(workspace_id, channel_id, cx);
                    });
                }));
                rows.push(
                    div()
                        .flex_none()
                        .when(after_occupants, |wrapper| wrapper.mt(u(4.0)))
                        // The identifier and not the name: a name comes from the server
                        // and need not be the only one.
                        .debug_selector(move || format!("channel-row:{channel_id}"))
                        .child(row)
                        .into_any_element(),
                );
                // The threads of the channel that hold something unread, and the thread
                // of the side panel.
                for (place, entry) in channel.threads.iter().enumerate() {
                    let root_id = entry.root_id;
                    let title: SharedString = entry
                        .title
                        .clone()
                        .unwrap_or_else(|| text::THREAD.to_owned())
                        .into();
                    let sign =
                        (entry.unread > 0 || entry.mentions > 0).then(|| crate::ui::UnreadSign {
                            mentions: (entry.mentions > 0)
                                .then(|| text::mention_badge(entry.mentions).into()),
                            label: text::unread_tooltip(entry.unread, entry.mentions).into(),
                        });
                    let row = ChannelRow::new(("thread", index * 1000 + place), Icon::Chat, title)
                        .nested()
                        .selected(entry.open)
                        .unread(sign)
                        .unavailable((!entry.available).then_some(text::THREAD_IN_CALL))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            let Some(server_id) = server_id else {
                                return;
                            };
                            this.state.update(cx, |state, cx| {
                                state.select_channel(workspace_id, channel_id, cx);
                            });
                            this.open_thread(server_id, channel_id, root_id, window, cx);
                        }));
                    rows.push(
                        div()
                            .flex_none()
                            .debug_selector(move || format!("thread-row:{root_id}"))
                            .child(row)
                            .into_any_element(),
                    );
                }
                let occupants = channel
                    .voice
                    .as_ref()
                    .map(|voice| voice.occupants.as_slice())
                    .unwrap_or_default();
                after_occupants = !occupants.is_empty();
                for (seat, occupant) in occupants.iter().enumerate() {
                    let account_id = occupant.account_id;
                    let menu = open_menu
                        .as_ref()
                        .filter(|(menu, _)| {
                            menu.channel_id == channel_id && menu.account_id == account_id
                        })
                        .map(|(menu, moving)| self.render_person_menu(menu, *moving, window, cx));
                    let row = OccupantRow::new(
                        ("occupant", index * 1000 + seat),
                        occupant_name(occupant),
                        account_id.as_uuid().as_u128(),
                    )
                    .marks(occupant_marks(occupant))
                    .selector(format!("occupant:{account_id}"))
                    .open(menu.is_some());
                    // The menu of a person holds what the person of this device can do with
                    // another person. The own row has none.
                    let row = if occupant.is_self {
                        row
                    } else {
                        row.on_menu(Self::handler(cx, move |this, window, cx| {
                            this.open_person_menu(channel_id, account_id, window, cx);
                        }))
                    };
                    rows.push(
                        div()
                            .relative()
                            .flex_none()
                            .child(row)
                            .children(menu)
                            .into_any_element(),
                    );
                }
                if let (Some(server_id), Some(voice)) = (server_id, channel.voice.as_ref())
                    && let Some(entry) = self.render_entry_box(server_id, channel, voice, theme, cx)
                {
                    after_occupants = true;
                    rows.push(entry);
                }
            }
        }
        if rows.is_empty() {
            rows.push(
                div()
                    .px(u(14.0))
                    .pt(u(12.0))
                    .typeface(theme.types.small, theme)
                    .ink(colors.ink_2)
                    .child(text::NO_CHANNELS)
                    .into_any_element(),
            );
        }
        // The direct conversations of the server, after the channels of the workspace.
        if !workspace.directs.is_empty() {
            rows.push(CategoryHeader::new(text::DIRECT_MESSAGES).into_any_element());
        }
        for (index, direct) in workspace.directs.iter().enumerate() {
            let conversation_id = direct.id;
            let name: SharedString = direct
                .name
                .clone()
                .unwrap_or_else(|| text::UNKNOWN_PERSON.to_owned())
                .into();
            let row = ChannelRow::new(("direct", index), Icon::People, name)
                .avatar(direct.account_id.as_uuid().as_u128())
                .selected(direct.selected)
                .unread(unread_sign(direct.unread))
                .muted(direct.unread.muted)
                .on_click(cx.listener(move |this, _, _, cx| {
                    let Some(server_id) = server_id else {
                        return;
                    };
                    this.state.update(cx, |state, cx| {
                        state.select_conversation(server_id, conversation_id, cx);
                    });
                }));
            rows.push(
                div()
                    .flex_none()
                    .debug_selector(move || format!("direct-row:{conversation_id}"))
                    .child(row)
                    .into_any_element(),
            );
        }
        ScrollArea::new(
            "channel-list",
            &self.channels_scroll,
            workspace.name.clone(),
        )
        .selector("channel-list")
        .child(div().flex_none().h(u(6.0)))
        .children(rows)
        .child(div().flex_none().h(u(8.0)))
    }

    fn render_profile_strip(
        &mut self,
        view: &ShellView,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = theme.colors;
        let name: SharedString = if view.person.name.is_empty() {
            text::NO_NAME_YET.into()
        } else {
            view.person.name.clone().into()
        };
        let seed = view
            .person
            .account_id
            .map_or_else(|| name_seed(&view.person.name), |id| id.as_uuid().as_u128());
        div()
            .id("profile-strip")
            .flex()
            .flex_none()
            .items_center()
            .gap(u(9.0))
            .h(u(theme.metrics.profile_strip_height))
            .pl(u(12.0))
            .pr(u(8.0))
            .bg(Hsla::from(colors.limestone_lo))
            .border_t_1()
            .border_color(Hsla::from(colors.rule))
            .child(Mosaic::new(MosaicKind::Large, seed, 30.0))
            .child(
                div()
                    .id("profile-name")
                    .debug_selector(|| "profile-name".to_owned())
                    .aria_label(name.clone())
                    .flex_1()
                    .min_w_0()
                    .typeface(theme.types.name, theme)
                    .ink(colors.ink)
                    .truncate()
                    .child(name),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .gap(u(4.0))
                    .child(
                        match view.controls.microphone {
                            MicrophoneButton::Open => Button::new("microphone")
                                .kind(ButtonKind::Square)
                                .icon(Icon::Microphone)
                                .tooltip(text::CLOSE_MICROPHONE),
                            MicrophoneButton::Muted => Button::new("microphone")
                                .kind(ButtonKind::SquareOff)
                                .icon(Icon::MicrophoneOff)
                                .tooltip(text::OPEN_MICROPHONE),
                            MicrophoneButton::Held => Button::new("microphone")
                                .kind(ButtonKind::SquareHeld)
                                .icon(Icon::MicrophoneHeld)
                                .tooltip(text::MICROPHONE_HELD),
                        }
                        .selector("microphone")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.state.update(cx, |state, _| state.toggle_microphone());
                        })),
                    )
                    .child(
                        if view.controls.sound_off {
                            Button::new("sound-output")
                                .kind(ButtonKind::SquareOff)
                                .icon(Icon::PhonesOff)
                                .tooltip(text::SOUND_OUTPUT_OFF)
                        } else {
                            Button::new("sound-output")
                                .kind(ButtonKind::Square)
                                .icon(Icon::Phones)
                                .tooltip(text::SOUND_OUTPUT_ON)
                        }
                        .selector("sound-output")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.state.update(cx, |state, _| state.toggle_sound());
                        })),
                    )
                    .child(
                        Button::new("app-settings")
                            .kind(ButtonKind::Square)
                            .icon(Icon::Gear)
                            .tooltip(text::APP_SETTINGS)
                            .selector("app-settings")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_settings(window, cx);
                            })),
                    ),
            )
    }
}

/// The seed of the avatar of a person who has no account yet: from the name.
fn name_seed(name: &str) -> u128 {
    name.bytes().fold(0x6a75_6e74_6f00_u128, |seed, byte| {
        seed.rotate_left(8) ^ u128::from(byte)
    })
}

/// The sign of a channel, a workspace or a server that holds what the person has not read.
pub(crate) fn unread_sign(unread: crate::state::view::Unread) -> Option<crate::ui::UnreadSign> {
    unread.any().then(|| crate::ui::UnreadSign {
        mentions: (unread.mentions > 0).then(|| text::mention_badge(unread.mentions).into()),
        label: text::unread_tooltip(unread.messages, unread.mentions).into(),
    })
}
