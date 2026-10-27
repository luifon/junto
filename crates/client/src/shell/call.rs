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
use junto_client_core::voice::{SHARE_FRAME_RATES, ShareResolution};
use junto_media::calls::Phase;
use junto_media::permission::MicrophoneAccess;

use super::channels::occupant_name;
use super::room::option_row;
use super::{Layer, Shell};
use crate::assets::Icon;
use crate::state::CameraAction;
use crate::state::view::{
    CallPanel, CallStrip, ChannelRow, DeviceChoice, INPUT_SEGMENTS, Microphone, MicrophoneLine,
    Occupant, OwnShare, PanelPlace, Presence, Relation, ShellView, SoundWarning, VoiceState,
};
use crate::text;
use crate::theme::{Theme, theme, u};
use crate::ui::{
    Button, ButtonKind, Flag, FlagKind, Menu, MenuItem, Meter, PersonTile, ScrollArea,
    SectionLabel, Segmented, SelectBox, StyleExt, Tab, TileMark, Toggle, VolumeSlider,
    float_shadow, icon, tooltip,
};

/// The width that a panel over the content adds for its close button, in pixels of the
/// design.
const PANEL_CLOSE_WIDTH: f32 = 40.0;

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
    Camera,
    Input,
    Output,
}

/// The volume control of one person: its value, its keyboard focus, and the subscription
/// that carries a change to the state of the application.
pub(crate) struct VolumeControl {
    pub(crate) state: Entity<SliderState>,
    pub(crate) focus: FocusHandle,
    pub(crate) _changes: Subscription,
}

/// A person outside the call in the member list of a call, with the line of the state.
struct OutsideRow {
    account_id: AccountId,
    name: SharedString,
    state: Option<(Icon, SharedString)>,
}

/// The marks on the plate of the tile of a person, and the words for them.
pub(crate) fn tile_marks(occupant: &Occupant) -> (Vec<TileMark>, String) {
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

impl Shell {
    /// The volume control of one person of the call. A control is made when its person is
    /// drawn first and follows the stored volume when that changes elsewhere.
    pub(crate) fn volume_control(
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

    /// The volume control of the sound of the watched share.
    fn share_volume_control(
        &mut self,
        percent: u16,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Entity<SliderState>, FocusHandle) {
        let control = self.share_volume.get_or_insert_with(|| {
            let state = cx.new(|_| VolumeSlider::state(percent));
            let changes = cx.subscribe(&state, move |this, _, event: &SliderEvent, cx| {
                let (value, release) = match event {
                    SliderEvent::Change(value) => (value.end(), false),
                    SliderEvent::Release(value) => (value.end(), true),
                };
                let percent = value.round().clamp(0.0, 200.0) as u16;
                this.state.update(cx, |state, cx| {
                    state.set_share_volume(percent, cx);
                    if release {
                        state.store_share_volume(cx);
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

    /// The buttons of the header that open the panel of the call at one tab, in a narrow
    /// window and in the call window.
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
                    this.call_panel_hidden = false;
                }))
                .into_any_element()
        })
        .collect()
    }

    /// The button of the call window, which has no channel column, that opens the panel of
    /// the call at the notices of the server of the call, with the count of those that the
    /// person has not read (ADR 0742). It is drawn also while the panel waits for the
    /// conversation of a direct-message call.
    pub(crate) fn render_call_notices_button(
        &mut self,
        view: &ShellView,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let unread = view.lists().map_or(0, |lists| lists.unread_notices);
        let label: SharedString = if unread > 0 {
            format!("{}, {}", text::NOTIFICATIONS, text::unread_notices(unread)).into()
        } else {
            text::NOTIFICATIONS.into()
        };
        Button::new("open-call-notices")
            .icon(Icon::Bell)
            .tooltip(text::NOTIFICATIONS)
            .accessible_label(label)
            .count((unread > 0).then(|| text::unread_count(unread as u64)))
            .selector("open-call-notices")
            .on_click(cx.listener(|this, _, _, cx| {
                this.open_call_notices(cx);
                this.call_panel_open = true;
                this.call_panel_hidden = false;
                cx.notify();
            }))
            .into_any_element()
    }

    /// Shows the notices of the server of the call in the panel of the call.
    pub(crate) fn open_call_notices(&mut self, cx: &mut Context<Self>) {
        if self.detached {
            self.state
                .update(cx, |state, cx| state.show_call_window_notices(true, cx));
            return;
        }
        let server_id = self
            .view
            .as_ref()
            .and_then(|view| view.selected.as_ref())
            .map(|server| server.id);
        if let Some(server_id) = server_id {
            self.members_open = false;
            self.state
                .update(cx, |state, cx| state.open_notices(server_id, cx));
        }
    }

    /// The panel of the call shows a tab again in place of the notices.
    pub(crate) fn close_call_notices(&mut self, cx: &mut Context<Self>) {
        if self.detached {
            self.state
                .update(cx, |state, cx| state.show_call_window_notices(false, cx));
            return;
        }
        let in_call_with_notices = self
            .view
            .as_ref()
            .and_then(|view| view.lists())
            .is_some_and(|lists| lists.call.is_some() && lists.notices.is_some());
        if in_call_with_notices {
            self.state.update(cx, |state, cx| state.close_notices(cx));
        }
    }

    pub(crate) fn show_call_tab(&mut self, tab: CallTab, cx: &mut Context<Self>) {
        self.close_call_notices(cx);
        self.call_tab = tab;
        if tab != CallTab::Members {
            self.invite_open = false;
        }
        if tab == CallTab::Settings {
            self.state.update(cx, |state, cx| state.refresh_devices(cx));
        }
        cx.notify();
    }

    /// The stage of the call: the people as tiles, or what the call waits for.
    pub(crate) fn render_stage(
        &self,
        panel: &CallPanel,
        theme: &Theme,
        shell: gpui::WeakEntity<Self>,
        cx: &gpui::App,
    ) -> AnyElement {
        let colors = theme.colors;
        let faces = self.shown_faces();
        let people = panel.people.as_slice();
        // A direct-message call rings until the other person takes a place in it.
        let ringing = panel.direct().filter(|direct| !direct.joined);
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
            .id("camera-viewport")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .items_center()
            .justify_start()
            .overflow_y_scroll()
            .p(u(12.0));
        let note = |sentence: String| {
            div()
                .typeface(theme.types.small, theme)
                .text_size(u(13.0))
                .line_height(u(18.0))
                .font_weight(gpui::FontWeight::MEDIUM)
                .ink(colors.bone_2)
                .child(sentence)
        };
        let body = match panel.phase {
            Phase::Entering => viewport.justify_center().child(waiting(
                text::CALL_ENTERING,
                text::CALL_ENTERING_HELP.to_owned(),
                "call-entering",
            )),
            Phase::Connecting => viewport.justify_center().child(waiting(
                text::CONNECTING_TO_CALL,
                text::CONNECTING_TO_CALL_HELP.to_owned(),
                "call-connecting",
            )),
            Phase::Reconnecting => viewport.justify_center().child(waiting(
                text::CALL_RECONNECTING,
                text::connection_dropped(panel.interrupted_at, panel.place_name()),
                "call-lost",
            )),
            Phase::Connected => {
                let ids: Vec<_> = people.iter().map(|p| p.account_id).collect();
                let cameras: Vec<_> = people
                    .iter()
                    .filter(|p| self.camera_image(p).is_some())
                    .map(|p| p.account_id)
                    .collect();
                let hidden = super::camera::hidden_people(
                    &ids,
                    &self.people_with_video(people, cx),
                    panel.hide_without_video,
                );
                let speakers: Vec<_> = people
                    .iter()
                    .filter(|p| matches!(p.microphone, Microphone::Open { speaking: true }))
                    .map(|p| p.account_id)
                    .collect();
                let focus = if self.camera_focus {
                    super::camera::focused_camera(self.focused_camera.get(), &cameras, &speakers)
                } else {
                    None
                };
                self.focused_camera.set(focus);
                let shown: Vec<&Occupant> = people
                    .iter()
                    .filter(|p| !hidden.contains(&p.account_id))
                    .collect();
                let area = self.stage_area.get().unwrap_or((
                    crate::ui::tile::TILE_SIZE.0 * 2.0 + crate::ui::tile::TILE_GAP,
                    crate::ui::tile::TILE_SIZE.1,
                ));
                // The focused tile fills the stage above one row of the size of the strip.
                let others = shown.len().saturating_sub(usize::from(focus.is_some()));
                let (columns, size) = match focus {
                    Some(_) if others > 0 => crate::ui::tile::grid(
                        1,
                        (
                            area.0,
                            area.1 - crate::ui::tile::STRIP_TILE_SIZE.1 - crate::ui::tile::TILE_GAP,
                        ),
                    ),
                    Some(_) => crate::ui::tile::grid(1, area),
                    None => crate::ui::tile::grid(shown.len(), area),
                };
                let tile = |index: usize, occupant: &Occupant| {
                    let (marks, label) = tile_marks(occupant);
                    let name: SharedString = if occupant.is_self {
                        format!("{} ({})", occupant_name(occupant), text::YOU).into()
                    } else {
                        occupant_name(occupant)
                    };
                    let focused = focus == Some(occupant.account_id);
                    PersonTile::new(
                        ("tile", index),
                        name,
                        occupant.account_id.as_uuid().as_u128(),
                        label,
                    )
                    .map(|tile| {
                        if focus.is_some() && !focused {
                            tile.in_strip()
                        } else {
                            tile.size(size)
                        }
                    })
                    .focused(focused)
                    .picture(faces.image(occupant.account_id))
                    .camera_view(self.camera_request(occupant, cx))
                    .camera_picture(self.camera_image(occupant))
                    .speaking(matches!(
                        occupant.microphone,
                        Microphone::Open { speaking: true }
                    ))
                    .marks(marks)
                    .selector(format!("call-tile:{}", occupant.account_id))
                };
                let (large, strip): (Vec<_>, Vec<_>) = shown
                    .iter()
                    .enumerate()
                    .partition(|(_, p)| focus.is_none() || focus == Some(p.account_id));
                let width = match focus {
                    Some(_) => size.0,
                    None => {
                        columns as f32 * size.0
                            + crate::ui::tile::TILE_GAP * columns.saturating_sub(1) as f32
                    }
                };
                let measured = self.stage_area.clone();
                let shell = shell.clone();
                // The size of the free area of the stage, in design pixels: a new size draws
                // the tiles again at the size that fills it.
                let measure = gpui::canvas(
                    move |bounds, window, cx| {
                        let scale = f32::from(window.rem_size()) / 16.0;
                        let size = (
                            (f32::from(bounds.size.width) / scale).floor(),
                            (f32::from(bounds.size.height) / scale).floor(),
                        );
                        if size.0 <= 0.0 || measured.get() == Some(size) {
                            return;
                        }
                        measured.set(Some(size));
                        window.defer(cx, move |_, cx| {
                            let _ = shell.update(cx, |_, cx| cx.notify());
                        });
                    },
                    |_, (), _, _| {},
                )
                .absolute()
                .size_full();
                viewport.child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_h_0()
                        .w_full()
                        .items_center()
                        .child(
                            div()
                                .id("call-area")
                                .debug_selector(|| "call-area".to_owned())
                                .relative()
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_h_0()
                                .w_full()
                                .overflow_y_scroll()
                                .child(measure)
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .flex_none()
                                        .min_h_full()
                                        .w_full()
                                        .items_center()
                                        .justify_center()
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
                                                .gap(u(crate::ui::tile::TILE_GAP))
                                                .w(u(width))
                                                .max_w_full()
                                                .flex_none()
                                                .children(
                                                    large
                                                        .into_iter()
                                                        .map(|(index, p)| tile(index, p)),
                                                ),
                                        )
                                        .when(!strip.is_empty(), |area| {
                                            area.child(
                                                div()
                                                    .debug_selector(|| {
                                                        "call-focus-strip".to_owned()
                                                    })
                                                    .flex()
                                                    .flex_wrap()
                                                    .justify_center()
                                                    .gap(u(crate::ui::tile::TILE_GAP))
                                                    .mt(u(crate::ui::tile::TILE_GAP))
                                                    .w_full()
                                                    .flex_none()
                                                    .children(
                                                        strip
                                                            .into_iter()
                                                            .map(|(index, p)| tile(index, p)),
                                                    ),
                                            )
                                        }),
                                ),
                        )
                        .when_some(ringing, |viewport, direct| {
                            viewport.child(
                                div()
                                    .debug_selector(|| "direct-call-ringing".to_owned())
                                    .mt(u(14.0))
                                    .child(note(text::calling(direct.name.as_deref()))),
                            )
                        })
                        .when(ringing.is_none() && people.len() < 2, |viewport| {
                            viewport.child(
                                div()
                                    .debug_selector(|| "alone-in-call".to_owned())
                                    .mt(u(14.0))
                                    .child(note(text::ALONE_IN_CALL.to_owned())),
                            )
                        })
                        .children(
                            self.hidden_note(people, &hidden, theme)
                                .map(|note| note.mt(u(14.0))),
                        ),
                )
            }
        };
        div()
            .id("call-stage")
            .relative()
            .overflow_hidden()
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
                        .child(note(text::SOUND_RETURNS.to_owned())),
                )
            })
            .into_any_element()
    }

    /// The panel of a call, with the row of its three tabs.
    pub(crate) fn render_call_panel(
        &mut self,
        view: &ShellView,
        panel: &CallPanel,
        narrow: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = &theme(cx);
        let colors = theme.colors;
        // The notices of the server of the call take the place of the tab that is shown
        // (ADR 0742).
        let notices = view
            .lists()
            .and_then(|lists| lists.notices.as_ref())
            .zip(view.selected.as_ref().map(|server| server.id));
        let tab = |tab: CallTab,
                   shown: Icon,
                   label: &'static str,
                   selector: &'static str,
                   cx: &mut Context<Self>| {
            Tab::new(selector, shown, label)
                .on(self.call_tab == tab && notices.is_none())
                .selector(selector)
                .on_click(cx.listener(move |this, _, _, cx| this.show_call_tab(tab, cx)))
        };
        let tabs = div()
            .id("call-tabs")
            .role(Role::TabList)
            .aria_label(SharedString::from(panel.place_name().to_owned()))
            .flex()
            .flex_none()
            .items_stretch()
            .h(u(42.0))
            .px(u(8.0))
            .border_b_1()
            .border_color(Hsla::from(colors.rule))
            .bg(Hsla::from(colors.limestone))
            // The tabs give up their width before the close button does: the button
            // stays inside the panel.
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_stretch()
                    .overflow_hidden()
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
                    )),
            )
            .when(narrow, |tabs| {
                tabs.child(
                    div().flex().flex_none().items_center().child(
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
        let pane = match (&notices, self.call_tab) {
            (Some((shown, server_id)), _) => self.render_notices_panel(*server_id, shown, true, cx),
            (None, CallTab::Chat) => self.render_call_chat(view, panel, theme, window, cx),
            (None, CallTab::Members) => self.render_call_members(view, panel, theme, window, cx),
            (None, CallTab::Settings) => self.render_call_settings(panel, theme, window, cx),
        };
        div()
            .id("call-panel")
            .role(Role::Complementary)
            .aria_label(SharedString::from(panel.place_name().to_owned()))
            .debug_selector(|| "call-panel".to_owned())
            .flex()
            .flex_col()
            .flex_none()
            // A panel over the content also holds its close button beside the tabs.
            .w(u(
                theme.metrics.call_panel_width + if narrow { PANEL_CLOSE_WIDTH } else { 0.0 }
            ))
            .h_full()
            .bg(Hsla::from(colors.plaster))
            .border_l_1()
            .border_color(Hsla::from(colors.rule))
            .child(tabs)
            .child(pane)
            .into_any_element()
    }

    /// The notices of the server of the call in the place of the panel of the call, while
    /// the call window has no panel: before the state holds the conversation of a
    /// direct-message call. Absent while the notices are not open.
    pub(crate) fn render_call_notices_alone(
        &mut self,
        view: &ShellView,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let shown = view.lists()?.notices.as_ref()?;
        let server_id = view.selected.as_ref()?.id;
        let theme = theme(cx);
        let colors = theme.colors;
        let list = self.render_notices_panel(server_id, shown, true, cx);
        Some(
            div()
                .id("call-panel")
                .role(Role::Complementary)
                .aria_label(text::NOTIFICATIONS)
                .debug_selector(|| "call-panel".to_owned())
                .flex()
                .flex_col()
                .flex_none()
                .w(u(theme.metrics.call_panel_width))
                .h_full()
                .bg(Hsla::from(colors.plaster))
                .border_l_1()
                .border_color(Hsla::from(colors.rule))
                .child(list)
                .into_any_element(),
        )
    }

    /// The chat of the call in the panel of the call: the messages of the voice channel, or
    /// of the direct conversation of a direct-message call, and the composer.
    fn render_call_chat(
        &mut self,
        view: &ShellView,
        panel: &CallPanel,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = theme.colors;
        let feed = match view.lists() {
            Some(lists) => self.render_feed(lists, true, Vec::new(), window, cx),
            None => div().flex_1().into_any_element(),
        };
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
                            .child(panel.place_name().to_owned()),
                    )
                    .child(match panel.place {
                        PanelPlace::Channel(_) => text::VOICE_CHANNEL_CHAT,
                        PanelPlace::Direct(_) => text::DIRECT_CALL_CHAT,
                    }),
            )
            .child(feed)
            .into_any_element()
    }

    fn render_call_members(
        &mut self,
        view: &ShellView,
        panel: &CallPanel,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = theme.colors;
        let server_id = view.selected.as_ref().map(|server| server.id);
        let faces = self.shown_faces();
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
                .child(
                    div()
                        .mt(u(2.0))
                        .child(super::people::avatar(&faces, account_id, 30.0)),
                )
        };
        let name_line = |name: SharedString, is_self: bool, camera: Option<AccountId>| {
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
                .when_some(camera, |line, account| {
                    line.child(
                        div()
                            .id(format!("member-camera:{account}"))
                            .debug_selector(move || format!("member-camera:{account}"))
                            .tooltip(tooltip(text::CAMERA))
                            .child(icon(Icon::Camera, 14.0, colors.ink_2)),
                    )
                })
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
        let people = panel.people.as_slice();
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
                                name_line(
                                    name,
                                    occupant.is_self,
                                    self.camera_request(occupant, cx)
                                        .map(|_| occupant.account_id),
                                )
                                .child(div().flex_1())
                                .child(div().flex().flex_none().items_center().child(mark)),
                            )
                            .child(state_line(member_states(occupant)))
                            .children(volume),
                    )
                    .into_any_element(),
            );
        }
        // Outside the call: the members of the workspace, or the other person of a
        // direct-message call while it rings.
        let outside: Vec<OutsideRow> = match &panel.place {
            PanelPlace::Channel(channel) => channel
                .outside
                .iter()
                .map(|member| OutsideRow {
                    account_id: member.account_id,
                    name: member.name.clone().into(),
                    state: member
                        .place
                        .as_ref()
                        .map(|place| (Icon::Speaker, text::in_channel(place).into())),
                })
                .collect(),
            PanelPlace::Direct(direct) if !direct.joined => vec![OutsideRow {
                account_id: direct.other,
                name: panel.place_name().to_owned().into(),
                state: Some((Icon::PhoneOutgoing, text::CALLING.into())),
            }],
            PanelPlace::Direct(_) => Vec::new(),
        };
        let outside: Vec<AnyElement> = outside
            .into_iter()
            .enumerate()
            .map(
                |(
                    index,
                    OutsideRow {
                        account_id,
                        name,
                        state,
                    },
                )| {
                    row(people.len() + index, account_id, name.clone())
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(name_line(name, false, None))
                                .when_some(state, |column, state| {
                                    column.child(state_line(vec![state]))
                                }),
                        )
                        .into_any_element()
                },
            )
            .collect();
        let list = ScrollArea::new("call-members", &self.call_members_scroll, text::MEMBERS)
            .selector("call-members")
            .child(SectionLabel::new(text::IN_THE_CALL).count(people.len()))
            .children(inside)
            .when(!outside.is_empty(), |list| {
                list.child(SectionLabel::new(text::NOT_IN_THE_CALL).count(outside.len()))
                    .children(outside)
            })
            .child(div().flex_none().h(u(4.0)));
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(list)
            .children(
                panel
                    .channel()
                    .map(|channel| self.render_invite_area(channel, theme, cx)),
            )
            .into_any_element()
    }

    fn render_call_settings(
        &mut self,
        panel: &CallPanel,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = theme.colors;
        let audio = &panel.audio;
        // A row with a name and a control of a few choices.
        let choice_row = |label: &'static str, control: Segmented| {
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(u(10.0))
                .py(u(3.0))
                .px(u(14.0))
                .child(
                    div()
                        .flex_none()
                        .w(u(84.0))
                        .typeface(theme.types.control, theme)
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .ink(colors.ink)
                        .child(label),
                )
                .child(control)
        };
        let resolutions = [
            ShareResolution::P720,
            ShareResolution::P1080,
            ShareResolution::Source,
        ];
        let resolution = choice_row(
            text::RESOLUTION,
            Segmented::new(
                "share-resolution",
                text::RESOLUTION,
                resolutions
                    .map(|resolution| SharedString::from(text::share_resolution(resolution))),
            )
            .selected(
                resolutions
                    .iter()
                    .position(|resolution| *resolution == panel.share_resolution),
            )
            .selector("share-resolution")
            .on_select({
                let select = cx.listener(move |this, index: &usize, _, cx| {
                    if let Some(resolution) = resolutions.get(*index).copied() {
                        this.state
                            .update(cx, |state, cx| state.set_share_resolution(resolution, cx));
                    }
                });
                move |index, window, cx| select(&index, window, cx)
            }),
        );
        let frame_rate = choice_row(
            text::FRAME_RATE,
            Segmented::new(
                "share-frame-rate",
                text::FRAME_RATE,
                SHARE_FRAME_RATES.map(|rate| SharedString::from(rate.to_string())),
            )
            .selected(
                SHARE_FRAME_RATES
                    .iter()
                    .position(|rate| *rate == panel.share_frame_rate),
            )
            .selector("share-frame-rate")
            .on_select({
                let select = cx.listener(move |this, index: &usize, _, cx| {
                    if let Some(rate) = SHARE_FRAME_RATES.get(*index).copied() {
                        this.state
                            .update(cx, |state, cx| state.set_share_frame_rate(rate, cx));
                    }
                });
                move |index, window, cx| select(&index, window, cx)
            }),
        );
        let hide = panel.hide_without_video;
        let hide_row = option_row(
            text::HIDE_WITHOUT_VIDEO,
            text::HIDE_WITHOUT_VIDEO_HELP,
            Toggle::new(
                "hide-without-video",
                text::HIDE_WITHOUT_VIDEO,
                hide,
                (text::ON, text::OFF),
            )
            .selector("hide-without-video")
            .on_click(cx.listener(move |this, _, _, cx| {
                this.state
                    .update(cx, |state, cx| state.set_hide_without_video(!hide, cx));
            }))
            .into_any_element(),
            theme,
        );
        let room_rows = match panel.room() {
            Some(room) => self.render_room_settings(room, theme, cx),
            None => Vec::new(),
        };
        let share_volume = self.state.read(cx).voice().share_volume;
        let (volume_state, volume_focus) = self.share_volume_control(share_volume, window, cx);
        let volume_row = div()
            .flex_none()
            .pt(u(5.0))
            .pb(u(6.0))
            .px(u(14.0))
            .child(
                div()
                    .typeface(theme.types.control, theme)
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .line_height(u(19.0))
                    .ink(colors.ink)
                    .child(text::SHARE_VOLUME),
            )
            .child(
                div().mt(u(3.0)).child(
                    VolumeSlider::new(
                        "share-volume",
                        &volume_state,
                        &volume_focus,
                        text::SHARE_VOLUME,
                    )
                    .selector("share-volume"),
                ),
            )
            .child(
                div()
                    .mt(u(2.0))
                    .typeface(theme.types.small, theme)
                    .line_height(u(17.0))
                    .ink(colors.ink_2)
                    .child(text::SHARE_VOLUME_HELP),
            );
        let help_line = |sentence: &'static str| {
            div()
                .flex_none()
                .px(u(14.0))
                .pb(u(4.0))
                .typeface(theme.types.small, theme)
                .line_height(u(17.0))
                .ink(colors.ink_2)
                .child(sentence)
        };
        let shown = |kind: DeviceKind, choices: &[DeviceChoice]| -> SharedString {
            let chosen = choices.iter().find(|choice| choice.selected);
            match chosen {
                Some(choice) if choice.id.is_none() && kind == DeviceKind::Camera => {
                    super::camera::default_name().into()
                }
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
                            SelectBox::new(selector, label, shown(kind, choices))
                                .selector(selector)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.open_device_menu(kind, window, cx);
                                })),
                        )
                        .children(menu),
                )
        };
        let camera_choices = self.state.read(cx).camera_choices();
        let camera = field(
            DeviceKind::Camera,
            text::CAMERA,
            &camera_choices,
            "camera-device",
            self,
            cx,
        );
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
        let seat = self.drawn_seat();
        // The question of the system, or the page of its settings (ADR 0708).
        let microphone_permission = self.permission_button(
            "microphone-permission-action",
            crate::permissions::Permission::Microphone,
            cx,
        );
        ScrollArea::new("call-settings", &self.call_settings_scroll, text::SETTINGS)
            .selector("call-settings")
            .child(camera)
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
                    .when_some(microphone_permission, |column, button| {
                        column.child(div().flex().child(button))
                    })
                    .when(unusable, |column| {
                        column.child(
                            div().flex().child(
                                Button::new("retry-microphone")
                                    .label(text::TRY_AGAIN)
                                    .icon(Icon::Retry)
                                    .selector("retry-microphone")
                                    .on_click({
                                        let seat = seat.clone();
                                        cx.listener(move |this, _, _, cx| {
                                            this.state.update(cx, |state, cx| {
                                                state.retry_microphone(seat.as_ref(), cx);
                                            });
                                        })
                                    }),
                            ),
                        )
                    })
                    .child(small(text::MORE_SOUND_SETTINGS.into())),
            )
            .child(SectionLabel::new(text::YOUR_SCREEN_SHARE))
            .child(resolution)
            .child(frame_rate)
            .child(help_line(text::SHARE_SETTINGS_HELP))
            .child(SectionLabel::new(text::YOUR_VIEW))
            .child(hide_row)
            .child(volume_row)
            .children(room_rows)
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
            DeviceKind::Camera => text::CAMERA,
            DeviceKind::Input => text::INPUT_DEVICE,
            DeviceKind::Output => text::OUTPUT_DEVICE,
        };
        let items: Vec<AnyElement> = choices
            .iter()
            .enumerate()
            .map(|(index, choice)| {
                let id = choice.id.clone();
                let name: SharedString = match (&choice.id, choice.name.is_empty()) {
                    (None, _) if kind == DeviceKind::Camera => super::camera::default_name().into(),
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
                                    DeviceKind::Camera => Icon::Camera,
                                    DeviceKind::Input => Icon::Microphone,
                                    DeviceKind::Output => Icon::Phones,
                                }
                            })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                let id = id.clone();
                                this.state.update(cx, |state, cx| match kind {
                                    DeviceKind::Camera => state.choose_camera(id, cx),
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
        // The input mode is stated while it is push to talk: the microphone sends nothing
        // until the key is held.
        // The keys of push to talk as they work now: the binding of the person, or the key
        // of the sound settings (ADR 0777).
        let push_to_talk = {
            let state = self.state.read(cx);
            (!interrupted
                && state.voice().input_mode == junto_client_core::voice::InputMode::PushToTalk)
                .then(|| state.talk_keys())
                .flatten()
        };
        let (ground, ink) = if interrupted {
            (colors.ochre, colors.ink)
        } else {
            (colors.cypress, colors.call_text)
        };
        let leave = |cx: &mut Context<Self>| {
            Button::new("leave-call")
                .kind(ButtonKind::CallLeave)
                .icon(Icon::PhoneOff)
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
            Phase::Entering => text::CALL_ENTERING,
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
            // The hand of the person. A direct-message call has no meeting mode and no
            // hand (ADR 0505).
            let hand = strip.hand_raised.map(|raised| {
                Button::new("call-hand")
                    .kind(if raised {
                        ButtonKind::CallOn
                    } else {
                        ButtonKind::Call
                    })
                    .icon(Icon::Hand)
                    .tooltip(if raised {
                        text::LOWER_HAND
                    } else {
                        text::RAISE_HAND
                    })
                    .selector("call-hand")
                    .on_click(self.call_listener(cx, move |this, _, _, cx| {
                        this.state
                            .update(cx, |state, cx| state.set_hand(!raised, cx));
                    }))
            });
            let share = match (strip.can_share, strip.share) {
                (false, _) => Button::new("call-share")
                    .kind(ButtonKind::Call)
                    .icon(Icon::Screen)
                    .tooltip(text::SHARE_NOT_ON_THIS_SYSTEM)
                    .disabled(true),
                (true, OwnShare::Off) => Button::new("call-share")
                    .kind(ButtonKind::Call)
                    .icon(Icon::Screen)
                    .tooltip(text::SHARE_SCREEN)
                    .accessible_label(text::SHARE_SCREEN),
                (true, OwnShare::Starting) => Button::new("call-share")
                    .kind(ButtonKind::CallOn)
                    .icon(Icon::ScreenOff)
                    .tooltip(text::SHARE_STARTING)
                    .accessible_label(text::SHARE_STARTING),
                (true, OwnShare::Running) => Button::new("call-share")
                    .kind(ButtonKind::CallOn)
                    .icon(Icon::ScreenOff)
                    .tooltip(text::SHARE_RUNS)
                    .accessible_label(text::SHARE_RUNS),
            }
            .selector("call-share")
            .on_click(cx.listener(|this, _, window, cx| this.toggle_share(window, cx)));
            let share_menu = matches!(self.layer, Layer::ShareMenu)
                .then(|| self.render_share_menu(&strip.own_shares, strip.share_limit, cx));
            let share = div()
                .relative()
                .flex_none()
                .child(share)
                .children(share_menu);
            // One reading of the calls gives the look of the camera control and what it
            // does: the control does what it shows, also when the call is another one at
            // the click (ADR 0788).
            let camera_call = self.state.read(cx).media_calls().call;
            let camera = camera_call.as_ref().map(CameraAction::drawn_for);
            let camera_on = camera == Some(CameraAction::Stop);
            let camera_usable = camera_on
                || camera_call
                    .as_ref()
                    .is_some_and(|call| call.phase == junto_media::calls::Phase::Connected);
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(u(5.0))
                .mt(u(7.0))
                .child(
                    Button::new("call-camera")
                        .kind(if camera_on {
                            ButtonKind::CallOn
                        } else {
                            ButtonKind::Call
                        })
                        .icon(if camera_on {
                            Icon::Camera
                        } else {
                            Icon::CameraOff
                        })
                        .tooltip(text::CAMERA)
                        .accessible_label(text::CAMERA)
                        .selector("call-camera")
                        .disabled(!camera_usable)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(action) = &camera {
                                this.state
                                    .update(cx, |state, cx| state.use_camera(action, cx));
                            }
                        })),
                )
                .child(
                    Button::new("camera-focus")
                        .kind(if self.camera_focus {
                            ButtonKind::CallOn
                        } else {
                            ButtonKind::Call
                        })
                        .icon(Icon::LayoutGrid)
                        .selector("camera-focus")
                        .tooltip(if self.camera_focus {
                            "Show camera grid"
                        } else {
                            text::FOCUS_SPEAKER
                        })
                        .accessible_label(if self.camera_focus {
                            "Show camera grid"
                        } else {
                            text::FOCUS_SPEAKER
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.camera_focus = !this.camera_focus;
                            cx.notify();
                        })),
                )
                .child(share)
                .when(
                    !self.detached && !self.state.read(cx).is_call_detached(),
                    |row| {
                        row.child(
                            Button::new("detach-call")
                                .icon(Icon::Detach)
                                .accessible_label(text::DETACH_CALL)
                                .tooltip(text::DETACH_CALL)
                                .selector("detach-call")
                                .on_click(cx.listener(|this, _, _, cx| this.detach_call(cx))),
                        )
                    },
                )
                .children(hand)
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
            .when_some(push_to_talk, |strip, key| {
                strip.child(
                    div()
                        .id("input-mode-mark")
                        .role(Role::Status)
                        .aria_label(text::push_to_talk_mark(&key))
                        .debug_selector(|| "input-mode-mark".to_owned())
                        .tooltip(tooltip(text::PUSH_TO_TALK_MARK_HELP))
                        .flex()
                        .items_center()
                        .gap(u(6.0))
                        .mt(u(5.0))
                        .typeface(theme.types.small, theme)
                        .line_height(u(16.0))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .child(icon(Icon::MicrophoneHeld, 13.0, ink))
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .child(text::push_to_talk_mark(&key)),
                        ),
                )
            })
            .child(second)
            .into_any_element()
    }

    pub(crate) fn render_camera_notice(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.state
            .read(cx)
            .media_calls()
            .call
            .and_then(|call| {
                super::camera::failure(&call.camera).map(|line| {
                    let refused = matches!(
                        call.camera,
                        junto_media::camera::CameraState::Failed(
                            junto_media::camera::CameraError::NoPermission(_)
                        )
                    );
                    (line, refused)
                })
            })
            .map(|(line, refused)| {
                // A refusal of the system has the page of its settings (ADR 0708).
                let permission = if refused {
                    self.permission_button(
                        "camera-permission-action",
                        crate::permissions::Permission::Camera,
                        cx,
                    )
                } else {
                    None
                };
                crate::ui::Notice::new(line)
                    .selector("camera-failed")
                    .stacked()
                    .action(
                        div()
                            .flex()
                            .gap(u(6.0))
                            .children(permission)
                            .child(
                                Button::new("camera-settings")
                                    .label(text::SETTINGS)
                                    .selector("camera-settings")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.open_settings(window, cx)
                                    })),
                            )
                            .child(
                                Button::new("camera-dismiss")
                                    .label(text::DISMISS)
                                    .selector("camera-dismiss")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.state.read(cx).calls().stop_camera();
                                    })),
                            ),
                    )
            })
            .map(IntoElement::into_any_element)
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
        let seat = self.drawn_seat();
        let shown = match warning {
            SoundWarning::MicrophoneHeld { .. } => Icon::MicrophoneHeld,
            SoundWarning::NoMicrophoneAccess | SoundWarning::MicrophoneFailed => {
                Icon::MicrophoneOff
            }
            SoundWarning::SoundHeld { .. } | SoundWarning::OutputFailed => Icon::PhonesOff,
        };
        let permission = if matches!(warning, SoundWarning::NoMicrophoneAccess) {
            self.permission_button(
                "warning-microphone-permission-action",
                crate::permissions::Permission::Microphone,
                cx,
            )
        } else {
            None
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
                            div()
                                .flex()
                                .gap(u(6.0))
                                .mt(u(6.0))
                                .children(permission)
                                .child(
                                    Button::new("warning-retry-microphone")
                                        .label(text::TRY_AGAIN)
                                        .icon(Icon::Retry)
                                        .selector("warning-retry-microphone")
                                        .on_click({
                                            let seat = seat.clone();
                                            cx.listener(move |this, _, _, cx| {
                                                this.state.update(cx, |state, cx| {
                                                    state.retry_microphone(seat.as_ref(), cx);
                                                });
                                            })
                                        }),
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
        panel: &CallPanel,
        narrow: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (AnyElement, Option<AnyElement>) {
        let theme = theme(cx);
        let stage = if self.state.read(cx).is_call_detached() && !self.detached {
            div()
                .flex()
                .flex_col()
                .flex_1()
                .items_center()
                .justify_center()
                .child(
                    crate::ui::Notice::new(text::CALL_SEPARATE).selector("call-in-separate-window"),
                )
                .child(self.return_call_button(cx))
                .into_any_element()
        } else {
            self.render_call_stage(panel, &theme, cx)
        };
        let side = (if narrow {
            self.call_panel_open
        } else {
            !self.call_panel_hidden
        })
        .then(|| self.render_call_panel(view, panel, narrow, window, cx));
        (stage, side)
    }

    /// The stage of a call: its shares while the call is connected and a person shares,
    /// otherwise its people.
    pub(crate) fn render_call_stage(
        &mut self,
        panel: &CallPanel,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match &panel.stage {
            Some(shares) if share_visible(panel.phase) => {
                self.render_share_stage(panel, shares, theme, cx)
            }
            _ => self.render_stage(panel, theme, cx.weak_entity(), cx),
        }
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

/// A share is drawn while the call is connected. A call that is restored keeps the pictures
/// of its shares and does not draw them.
fn share_visible(phase: Phase) -> bool {
    phase == Phase::Connected
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reconnecting_call_does_not_show_its_retained_share() {
        assert!(share_visible(Phase::Connected));
        assert!(!share_visible(Phase::Reconnecting));
        assert!(!share_visible(Phase::Connecting));
        assert!(!share_visible(Phase::Entering));
    }
}
