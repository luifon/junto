//! The channel column: the selected workspace of the selected server with its channels,
//! the people in its voice channels, and the profile strip of the person.

use gpui::{
    AnyElement, Context, Hsla, InteractiveElement, IntoElement, ParentElement, Role, SharedString,
    StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder,
};
use junto_client_core::protocol::structure::ChannelKind;

use super::Shell;
use crate::assets::Icon;
use crate::state::view::{
    Content, Microphone, Occupant, SectionTitle, ServerStatus, ShellView, VoiceState, WorkspaceView,
};
use crate::text;
use crate::theme::{Theme, theme, u};
use crate::ui::{
    Band, BandKind, Button, ButtonKind, CategoryHeader, ChannelRow, ChannelState, Mosaic,
    MosaicKind, OccupantMark, OccupantRow, ScrollArea, StyleExt, icon, tooltip,
};

/// The lit segments of the meter of a person who speaks. The server tells who speaks and
/// no level (ADR 0513); the media engine will give the level.
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
    marks.push(match occupant.microphone {
        Microphone::Held(reason) => {
            OccupantMark::MicrophoneHeld(text::microphone_held(reason).into())
        }
        Microphone::Closed => OccupantMark::MicrophoneOff(text::MICROPHONE_CLOSED.into()),
        Microphone::Open { speaking } => OccupantMark::Level {
            lit: if speaking { SPEAKING_SEGMENTS } else { 0 },
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
        _window: &mut Window,
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
                .render_channel_list(workspace, &theme, cx)
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
            .child(self.render_profile_strip(view, &theme, cx))
    }

    fn render_channel_list(
        &mut self,
        workspace: &WorkspaceView,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = theme.colors;
        let workspace_id = workspace.id;
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
                let occupants = channel
                    .voice
                    .as_ref()
                    .map(|voice| voice.occupants.as_slice())
                    .unwrap_or_default();
                after_occupants = !occupants.is_empty();
                for (seat, occupant) in occupants.iter().enumerate() {
                    rows.push(
                        OccupantRow::new(
                            ("occupant", index * 1000 + seat),
                            occupant_name(occupant),
                            occupant.account_id.as_uuid().as_u128(),
                        )
                        .marks(occupant_marks(occupant))
                        .into_any_element(),
                    );
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
                        Button::new("microphone")
                            .kind(ButtonKind::Square)
                            .icon(Icon::Microphone)
                            .disabled(true)
                            .tooltip(text::MICROPHONE_NOT_AVAILABLE),
                    )
                    .child(
                        Button::new("sound-output")
                            .kind(ButtonKind::Square)
                            .icon(Icon::Phones)
                            .disabled(true)
                            .tooltip(text::SOUND_NOT_AVAILABLE),
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
