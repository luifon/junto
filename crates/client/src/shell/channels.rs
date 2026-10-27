//! The channel column: the selected workspace of the selected server with its channels,
//! the people in its voice channels, and the profile strip of the person.

use gpui::{
    AnyElement, AppContext as _, Context, Hsla, InteractiveElement, IntoElement, ParentElement,
    Role, SharedString, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder,
};
use junto_client_core::protocol::ids::ServerId;
use junto_client_core::protocol::structure::ChannelKind;

use super::{Layer, Shell};
use crate::assets::Icon;
use crate::state::person_drop::PersonDrag;
use crate::state::view::{
    Microphone, MicrophoneButton, Occupant, Presence, SectionTitle, SelectedServer, ServerLists,
    ServerStatus, ShellView, VoiceState, WorkspaceView,
};
use crate::text;
use crate::theme::{Theme, theme, u};
use junto_client_core::settings::Panel;

use crate::ui::{
    Avatar, Band, BandKind, Button, ButtonKind, CategoryHeader, ChannelRow, ChannelState,
    OccupantMark, OccupantRow, ScrollArea, StyleExt, icon, tooltip,
};

/// The rows of direct conversations that the channel column shows at once when it has the
/// height. More scroll in their list.
const DIRECT_ROWS: f32 = 5.0;

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
    }
    // The count against a user limit, also before the channel is full (1 / 2).
    if let (Some(limit), false) = (voice.user_limit, voice.hidden) {
        states.push(ChannelState::Marked {
            icon: Icon::People,
            text: text::occupancy(voice.occupants.len(), limit, voice.full).into(),
            strong: voice.full,
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
        let header = super::title_bar::header_zone(div())
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

        let list: AnyElement = match (&view.selected, view.lists()) {
            (Some(server), Some(lists)) => self
                .render_channel_list(view, server, lists, &theme, window, cx)
                .into_any_element(),
            // Without the lists of a server the column says why it lists nothing.
            (selected, _) => div()
                .relative()
                .flex_1()
                .px(u(14.0))
                .pt(u(12.0))
                .typeface(theme.types.small, &theme)
                .ink(colors.ink_2)
                .debug_selector(|| "no-channel-list".to_owned())
                .child(if selected.is_some() {
                    text::CHANNELS_AFTER_ANSWER
                } else {
                    text::NO_SERVER_CHANNELS
                })
                .child(crate::ui::area("channel-column-empty"))
                .into_any_element(),
        };
        let stale = view
            .selected
            .as_ref()
            .is_some_and(|server| server.status == ServerStatus::Reconnecting);

        div()
            .id("channel-column")
            .debug_selector(|| "channel-column".to_owned())
            .relative()
            .flex()
            .flex_col()
            .flex_none()
            .w(u(self.panel_width(
                Panel::Channels,
                theme.metrics.channel_column_width,
                cx,
            )))
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
            .when_some(view.controls.warning.as_ref(), |column, warning| {
                column.child(self.render_sound_warning(warning, &theme, cx))
            })
            .children(self.render_update_notice(view, &theme, cx))
            .children(self.render_camera_notice(cx))
            .when_some(view.call.as_ref(), |column, strip| {
                column.child(self.render_call_strip(strip, &theme, cx))
            })
            .child(self.render_profile_strip(view, &theme, cx))
    }

    /// The lists of the selected server: the entry of its notices, the channels of the
    /// shown workspace, and its direct conversations. Without a workspace the column
    /// states that the account sees none, and keeps the notices and the direct
    /// conversations (ADR 0741).
    fn render_channel_list(
        &mut self,
        view: &ShellView,
        server: &SelectedServer,
        lists: &ServerLists,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = theme.colors;
        let workspace = lists.workspace.as_ref();
        let in_call = lists.call.is_some();
        let (channels, entries) = match workspace {
            Some(workspace) => self.channel_rows(view, workspace, theme, window, cx),
            None => (
                vec![
                    div()
                        .flex_none()
                        .px(u(14.0))
                        .pt(u(12.0))
                        .typeface(theme.types.small, theme)
                        .ink(colors.ink_2)
                        .debug_selector(|| "no-workspace-note".to_owned())
                        .child(text::NO_WORKSPACES)
                        .into_any_element(),
                ],
                Vec::new(),
            ),
        };
        let label: SharedString = match workspace {
            Some(workspace) => workspace.name.clone().into(),
            None => server.name.clone().into(),
        };
        // The part of the server, the notices and the direct conversations, has a height of
        // its own at the top of the column, and the lists of the workspace take the rest.
        // A change of the workspaces or of the channels moves none of its controls, and a
        // conversation that arrives moves no channel. In a short column its list of
        // conversations gives up rows down to one, and the channels keep one row too
        // (ADR 0730).
        let row = theme.metrics.channel_row_height;
        let server_part = div()
            .id("server-lists")
            .debug_selector(|| "server-lists".to_owned())
            .flex()
            .flex_col()
            .flex_grow_0()
            .flex_shrink_1()
            .pt(u(6.0))
            .pb(u(4.0))
            .border_b_1()
            .border_color(Hsla::from(colors.rule))
            .child(self.render_notices_row(lists, in_call, cx))
            .children(self.direct_rows(server.id, lists, theme, cx));
        let list = div().flex().flex_col().flex_1().min_h(u(row)).child(
            ScrollArea::new("channel-list", &self.channels_scroll, label)
                .selector("channel-list")
                .children(channels)
                .child(div().flex_none().h(u(8.0))),
        );
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(server_part)
            .child(list)
            // The entry boxes take their height from the end of the channel list, whose rows
            // stay in place. They scroll only when the column has no room for them, so the
            // action of a box stays in view at every width of the column (ADR 0739).
            .child(
                div()
                    .id("channel-entry-notices")
                    .debug_selector(|| "channel-entry-notices".to_owned())
                    .flex_grow_0()
                    .flex_shrink_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(entries),
            )
    }

    /// The sections of the channels of a workspace with their threads and the people in
    /// their voice channels, and the entry boxes of its voice channels.
    fn channel_rows(
        &mut self,
        view: &ShellView,
        workspace: &WorkspaceView,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Vec<AnyElement>, Vec<AnyElement>) {
        let colors = theme.colors;
        let workspace_id = workspace.id;
        let server_id = view.selected.as_ref().map(|server| server.id);
        let faces = self.shown_faces();
        // The person whose menu is open, and the menu.
        let open_menu = match &self.layer {
            Layer::PersonMenu(open) => view
                .person_menu(open.channel_id, open.account_id)
                .map(|menu| (menu, open.moving)),
            _ => None,
        };
        // The channel whose menu is open.
        let channel_menu = match &self.layer {
            Layer::ChannelMenu(channel_id) => Some(*channel_id),
            _ => None,
        };
        let mut rows: Vec<AnyElement> = Vec::new();
        let mut entries = Vec::new();
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
                .muted(channel.unread.muted || channel.mute.is_on())
                .open(channel_menu == Some(channel_id))
                .on_menu(Self::handler(cx, move |this, window, cx| {
                    this.open_channel_menu(channel_id, window, cx);
                }))
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
                let menu =
                    server_id
                        .filter(|_| channel_menu == Some(channel_id))
                        .map(|server_id| {
                            self.render_channel_menu(
                                server_id,
                                channel_id,
                                &channel.name,
                                &channel.mute,
                                cx,
                            )
                        });
                let held = self.state.clone();
                let hover = self.state.clone();
                rows.push(
                    div()
                        .id(("channel-drop", index))
                        .relative()
                        .flex_none()
                        .can_drop(move |value, _, cx| {
                            value.downcast_ref::<PersonDrag>().is_some_and(|person| {
                                held.read(cx)
                                    .view()
                                    .person_drop(*person, channel_id)
                                    .is_some()
                            })
                        })
                        .drag_over::<PersonDrag>(move |style, person, _, cx| {
                            if hover
                                .read(cx)
                                .view()
                                .person_drop(*person, channel_id)
                                .is_some()
                            {
                                style.bg(Hsla::from(colors.limestone_hover))
                            } else {
                                style
                            }
                        })
                        .on_drop(cx.listener(move |this, person: &PersonDrag, _, cx| {
                            this.drop_person(*person, channel_id, cx)
                        }))
                        .when(after_occupants, |wrapper| wrapper.mt(u(4.0)))
                        // The identifier and not the name: a name comes from the server
                        // and need not be the only one.
                        .debug_selector(move || format!("channel-row:{channel_id}"))
                        .child(row)
                        .children(menu)
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
                // The further threads of the shown channel that the person follows.
                if let Some(more) = channel.more_followed {
                    let row = ChannelRow::new(
                        ("followed-more", index),
                        Icon::ChevronDown,
                        text::followed::show_more(more.count),
                    )
                    .nested()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.state
                            .update(cx, |state, cx| state.show_more_followed(cx));
                    }));
                    rows.push(
                        div()
                            .flex_none()
                            .debug_selector(move || format!("followed-more:{channel_id}"))
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
                    .picture(faces.image(account_id))
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
                    let dragged =
                        (!occupant.is_self)
                            .then_some(server_id)
                            .flatten()
                            .map(|server_id| PersonDrag {
                                server_id,
                                channel_id,
                                account_id,
                            });
                    let name = occupant_name(occupant);
                    rows.push(
                        div()
                            .id(("person-drag", index * 1000 + seat))
                            .relative()
                            .flex_none()
                            .when_some(dragged, |row, person| {
                                row.on_drag(person, move |_, _, _, cx| {
                                    cx.new(|_| super::person_drag::DragLabel(name.clone()))
                                })
                            })
                            .child(row)
                            .children(menu)
                            .into_any_element(),
                    );
                }
                if let (Some(server_id), Some(voice)) = (server_id, channel.voice.as_ref())
                    && let Some(entry) = self.render_entry_box(server_id, channel, voice, theme, cx)
                {
                    entries.push(entry);
                }
            }
        }
        if channel_index == 0 {
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
        (rows, entries)
    }

    /// The section of the direct conversations of the server: its title, "New direct
    /// message", and the conversations in a list of a fixed height.
    fn direct_rows(
        &mut self,
        server_id: ServerId,
        lists: &ServerLists,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let faces = self.shown_faces();
        // The conversation whose menu is open.
        let direct_menu = match &self.layer {
            Layer::DirectMenu(conversation_id) => Some(*conversation_id),
            _ => None,
        };
        let mut rows = vec![CategoryHeader::new(text::DIRECT_MESSAGES).into_any_element()];
        // A new conversation, with an account that the server lists for the person
        // (ADR 0307). It stands before the conversations, so that a conversation that
        // arrives does not move it.
        rows.push(
            ChannelRow::new("new-direct", Icon::Plus, text::NEW_DIRECT)
                .selector("new-direct")
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_new_direct(server_id, window, cx);
                }))
                .into_any_element(),
        );
        let mut conversations: Vec<AnyElement> = Vec::new();
        for (index, direct) in lists.directs.iter().enumerate() {
            let conversation_id = direct.id;
            let name: SharedString = direct
                .name
                .clone()
                .unwrap_or_else(|| text::UNKNOWN_PERSON.to_owned())
                .into();
            let row = ChannelRow::new(("direct", index), Icon::Person, name)
                .avatar(direct.account_id.as_uuid().as_u128())
                .picture(faces.image(direct.account_id))
                .selected(direct.selected)
                .unread(unread_sign(direct.unread))
                .muted(direct.unread.muted || direct.mute.is_on())
                .open(direct_menu == Some(conversation_id))
                .on_menu(Self::handler(cx, move |this, window, cx| {
                    this.open_direct_menu(conversation_id, window, cx);
                }))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.state.update(cx, |state, cx| {
                        state.select_conversation(server_id, conversation_id, cx);
                    });
                }));
            let menu = (direct_menu == Some(conversation_id)).then(|| {
                let name = direct.name.as_deref().unwrap_or(text::UNKNOWN_PERSON);
                self.render_direct_menu(server_id, direct.account_id, name, &direct.mute, cx)
            });
            conversations.push(
                div()
                    .relative()
                    .flex_none()
                    .debug_selector(move || format!("direct-row:{conversation_id}"))
                    .child(row)
                    .children(menu)
                    .into_any_element(),
            );
        }
        if conversations.is_empty() {
            conversations.push(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .h(u(theme.metrics.channel_row_height))
                    .px(u(14.0))
                    .typeface(theme.types.small, theme)
                    .ink(theme.colors.ink_3)
                    .debug_selector(|| "no-direct-conversations".to_owned())
                    .child(text::NO_DIRECT_CONVERSATIONS)
                    .into_any_element(),
            );
        }
        // The list has the height of its rows up to five, and scrolls beyond them. A
        // conversation that arrives makes it taller when the pointer is not on a control
        // that it moves (ADR 0730).
        rows.push(
            crate::ui::held("direct-list-part")
                .scroll(&self.directs_scroll)
                .label(text::DIRECT_MESSAGES)
                .selector("direct-list")
                .most(u(theme.metrics.channel_row_height * DIRECT_ROWS))
                .flex_shrink_1()
                .min_h(u(theme.metrics.channel_row_height))
                .children(conversations)
                .child(crate::ui::area("direct-list"))
                .into_any_element(),
        );
        rows
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
        // An account shows its avatar on the selected server. Without an account the
        // person is shown as the profile preset describes them.
        let picture = match view.person.account_id {
            Some(account_id) => self.shown_faces().image(account_id),
            None => view.preset_picture.clone(),
        };
        div()
            .id("profile-strip")
            .debug_selector(|| "profile-strip".to_owned())
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
            .child(Avatar::new(seed, 30.0).image(picture))
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
                        .on_click({
                            // The control does what it shows now, also when the call
                            // is another one at the click (ADR 0788).
                            let action = view.controls.microphone_action.clone();
                            cx.listener(move |this, _, _, cx| {
                                this.state
                                    .update(cx, |state, _| state.use_microphone(&action));
                            })
                        }),
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
                        .on_click({
                            let off = !view.controls.sound_off;
                            cx.listener(move |this, _, _, cx| {
                                this.state.update(cx, |state, _| state.set_sound_off(off));
                            })
                        }),
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
pub(crate) fn name_seed(name: &str) -> u128 {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_user_limit_is_shown_whenever_it_is_set() {
        let people = |count: usize| {
            vec![
                Occupant {
                    account_id: junto_client_core::protocol::ids::AccountId::generate(),
                    name: None,
                    is_self: false,
                    microphone: Microphone::Closed,
                    held_by: None,
                    sharing: false,
                    hand_raised: false,
                    sound_off: false,
                    sound_blocked: false,
                    level: None,
                    presence: Presence::Here,
                    volume: 100,
                };
                count
            ]
        };
        let count = |voice: &VoiceState| {
            channel_states(voice)
                .into_iter()
                .find_map(|state| match state {
                    ChannelState::Marked {
                        icon: Icon::People,
                        text,
                        strong,
                    } => Some((text.to_string(), strong)),
                    _ => None,
                })
        };
        let mut voice = VoiceState {
            user_limit: Some(2),
            occupants: people(1),
            ..VoiceState::default()
        };
        assert_eq!(count(&voice), Some(("1 / 2".to_owned(), false)));
        voice.occupants = people(2);
        voice.full = true;
        assert_eq!(count(&voice), Some(("2 / 2 · full".to_owned(), true)));
        // A locked room shows the count next to the lock.
        voice.locked = true;
        assert!(count(&voice).is_some());
        // Without a limit, and while the room hides who is inside, no count.
        voice.user_limit = None;
        assert_eq!(count(&voice), None);
        voice.user_limit = Some(2);
        voice.hidden = true;
        voice.locked = false;
        assert_eq!(count(&voice), None);
    }
}
