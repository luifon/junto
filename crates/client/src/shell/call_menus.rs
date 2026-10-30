use gpui::{
    AnyElement, App, AppContext as _, Context, IntoElement, ParentElement, Styled, Window, div,
    prelude::FluentBuilder,
};
use gpui_base::slider::SliderEvent;

use super::{
    Layer, Shell,
    account_menus::{AccountSource, OpenAccountMenu},
    call::VolumeControl,
};
use crate::{
    text,
    theme::{theme, u},
    ui::{StyleExt, VolumeSlider},
};

fn share_sound_volume(open: &OpenAccountMenu) -> bool {
    matches!(
        &open.source,
        Some(AccountSource::CallOccupant {
            surface: super::account_menus::CallSurface::Share { watched: true, .. },
            ..
        })
    )
}

impl Shell {
    fn call_source_current(&self, open: &OpenAccountMenu, cx: &App) -> bool {
        matches!(&open.source, Some(AccountSource::CallOccupant { .. }))
            && self.account_context_current(open, cx)
    }

    pub(super) fn call_menu_current(&self, open: &OpenAccountMenu, cx: &App) -> bool {
        matches!(&self.layer, Layer::AccountMenu(current) if current.as_ref() == open)
            && self.call_source_current(open, cx)
    }

    pub(super) fn call_move_menu_current(&self, open: &OpenAccountMenu, cx: &App) -> bool {
        matches!(&self.layer, Layer::CallMoveMenu(current) if current.as_ref() == open)
            && self.call_source_current(open, cx)
    }

    fn call_volume_value(&self, open: &OpenAccountMenu) -> Option<u16> {
        let Some(AccountSource::CallOccupant { account_id, .. }) = &open.source else {
            return None;
        };
        if *account_id == open.owner.account_id {
            return None;
        }
        let panel = self.view.as_ref()?.lists()?.call.as_ref()?;
        if share_sound_volume(open) {
            let stage = panel.stage.as_ref()?;
            return stage
                .watched()
                .filter(|share| {
                    share.sound != Some(junto_client_core::protocol::call::ShareAudio::None)
                })
                .map(|_| stage.volume);
        }
        panel
            .people
            .iter()
            .find(|person| person.account_id == *account_id)
            .map(|person| person.volume)
    }

    pub(super) fn start_call_menu_volume(
        &mut self,
        open: &OpenAccountMenu,
        cx: &mut Context<Self>,
    ) {
        let Some(percent) = self.call_volume_value(open) else {
            return;
        };
        let context = open.clone();
        let state = cx.new(|_| VolumeSlider::state(percent));
        let changes = cx.subscribe(&state, move |this, _, event: &SliderEvent, cx| {
            if !this.call_menu_current(&context, cx) {
                return;
            }
            let (value, release) = match event {
                SliderEvent::Change(value) => (value.end(), false),
                SliderEvent::Release(value) => (value.end(), true),
            };
            let percent = value.round().clamp(0.0, 200.0) as u16;
            let account = context.people[0].account_id;
            this.state.update(cx, |state, cx| {
                if share_sound_volume(&context) {
                    state.set_share_volume(percent, cx);
                    if release {
                        state.store_share_volume(cx);
                    }
                } else {
                    state.set_volume(context.owner.entry.server_id, account, percent, cx);
                    if release {
                        state.store_volume(context.owner.entry.server_id, account, cx);
                    }
                }
            });
        });
        self.account_volume = Some(VolumeControl {
            state,
            focus: cx.focus_handle(),
            _changes: changes,
        });
    }

    pub(super) fn call_volume_row(
        &mut self,
        open: &OpenAccountMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let percent = self.call_volume_value(open)?;
        let control = self.account_volume.as_ref()?;
        if control.state.read(cx).value().end().round() as u16 != percent {
            control.state.update(cx, |state, cx| {
                state.set_value(f32::from(percent), window, cx)
            });
        }
        let theme = theme(cx);
        let name = &open.people[0].name;
        let label = if share_sound_volume(open) {
            text::SHARE_VOLUME.to_owned()
        } else {
            text::your_volume_for(name)
        };
        let accessible = if share_sound_volume(open) {
            text::SHARE_VOLUME.to_owned()
        } else {
            text::volume_for(name)
        };
        Some(
            div()
                .flex()
                .flex_col()
                .flex_none()
                .min_w_0()
                .px(u(12.0))
                .py(u(8.0))
                .gap(u(5.0))
                .typeface(theme.types.small, &theme)
                .child(div().truncate().child(label))
                .child(
                    div().flex().w_full().child(
                        VolumeSlider::new(
                            "call-menu-volume",
                            &control.state,
                            &control.focus,
                            accessible,
                        )
                        .selector("call-menu-volume"),
                    ),
                )
                .into_any_element(),
        )
    }
}

#[derive(Clone)]
enum CallMenuAction {
    Microphone(crate::state::view::MicrophoneAction),
    Sound(bool),
    Camera(crate::state::CameraAction),
    Hand(bool),
    Leave,
    WatchShare(junto_media::engine::ShareId),
    StopShare(u8),
    Participant(ParticipantAction),
}

#[derive(Clone)]
enum ParticipantAction {
    AllowSpeak,
    Invite,
    Withdraw(junto_client_core::protocol::ids::CallInvitationId),
    Microphone(bool),
    Sound(bool),
    LowerHand,
    Disconnect,
    Move,
}

struct CallControl {
    id: &'static str,
    label: &'static str,
    action: CallMenuAction,
    disabled: bool,
    detail: Option<String>,
}

impl Shell {
    pub(super) fn share_call_controls(
        &self,
        open: &OpenAccountMenu,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let Some(AccountSource::CallOccupant {
            account_id,
            surface: super::account_menus::CallSurface::Share { id, watched },
            ..
        }) = &open.source
        else {
            return Vec::new();
        };
        let Some(share) = self
            .view
            .as_ref()
            .and_then(|view| view.lists())
            .and_then(|lists| lists.call.as_ref())
            .and_then(|panel| panel.stage.as_ref())
            .and_then(|stage| stage.shares.iter().find(|share| share.id == *id))
        else {
            return Vec::new();
        };
        let mut items = Vec::new();
        if !watched {
            items.push(CallControl {
                id: "call-menu-watch-share",
                label: text::WATCH,
                action: CallMenuAction::WatchShare(*id),
                disabled: share.watched,
                detail: None,
            });
        }
        if *account_id == open.owner.account_id {
            items.push(CallControl {
                id: "call-menu-stop-share",
                label: text::STOP_SHARING,
                action: CallMenuAction::StopShare(id.slot),
                disabled: false,
                detail: None,
            });
        }
        self.call_controls(open, items, cx)
    }

    pub(super) fn own_call_controls(
        &self,
        open: &OpenAccountMenu,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        use crate::state::{
            CameraAction,
            view::{MicrophoneAction, MicrophoneButton},
        };
        use junto_client_core::protocol::WorkspacePermission;
        let Some(AccountSource::CallOccupant {
            seat, account_id, ..
        }) = &open.source
        else {
            return Vec::new();
        };
        if *account_id != open.owner.account_id || !self.call_menu_current(open, cx) {
            return Vec::new();
        }
        let Some(view) = &self.view else {
            return Vec::new();
        };
        let controls = view.controls.clone();
        if controls.seat.as_ref() != Some(seat.as_ref()) {
            return Vec::new();
        }
        let hand = view.call.as_ref().and_then(|call| call.hand_raised);
        let Some(call) = self.state.read(cx).shown_calls().call else {
            return Vec::new();
        };
        let camera = CameraAction::drawn_for(&call);
        let camera_on = matches!(&camera, CameraAction::Stop);
        let make = |id, label, action| CallControl {
            id,
            label,
            action,
            disabled: false,
            detail: None,
        };
        let mut microphone = make(
            "call-menu-microphone",
            if matches!(&controls.microphone_action, MicrophoneAction::Close) {
                text::CLOSE_MICROPHONE
            } else {
                text::OPEN_MICROPHONE
            },
            CallMenuAction::Microphone(controls.microphone_action),
        );
        microphone.disabled = controls.microphone == MicrophoneButton::NotPermitted;
        microphone.detail = match controls.microphone {
            MicrophoneButton::Held => Some(text::MICROPHONE_HELD.to_owned()),
            MicrophoneButton::NotPermitted => {
                Some(text::not_permitted(WorkspacePermission::Speak).to_owned())
            }
            _ => None,
        };
        let mut camera = make(
            "call-menu-camera",
            if camera_on {
                text::TURN_OFF_CAMERA
            } else {
                text::TURN_ON_CAMERA
            },
            CallMenuAction::Camera(camera),
        );
        camera.disabled = call.withheld.camera;
        camera.detail = call
            .withheld
            .camera
            .then(|| text::not_permitted(WorkspacePermission::Video).to_owned());
        let mut items = vec![
            microphone,
            make(
                "call-menu-sound",
                if controls.sound_off {
                    text::SOUND_OUTPUT_OFF
                } else {
                    text::SOUND_OUTPUT_ON
                },
                CallMenuAction::Sound(!controls.sound_off),
            ),
            camera,
        ];
        if let Some(raised) = hand {
            items.push(make(
                "call-menu-hand",
                if raised {
                    text::LOWER_HAND
                } else {
                    text::RAISE_HAND
                },
                CallMenuAction::Hand(!raised),
            ));
        }
        items.push(make(
            "call-menu-leave",
            text::LEAVE_CALL,
            CallMenuAction::Leave,
        ));
        self.call_controls(open, items, cx)
    }

    fn call_controls(
        &self,
        open: &OpenAccountMenu,
        items: Vec<CallControl>,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        items
            .into_iter()
            .map(|control| {
                let context = open.clone();
                crate::ui::MenuItem::new(control.id, control.label)
                    .selector(control.id)
                    .disabled(control.disabled)
                    .when(
                        matches!(
                            &control.action,
                            CallMenuAction::Participant(ParticipantAction::Move)
                        ),
                        |item| item.end_icon(crate::assets::Icon::ChevronRight),
                    )
                    .when_some(control.detail, |item, detail| item.detail(detail))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if !this.call_menu_current(&context, cx) {
                            return;
                        }
                        if matches!(
                            &control.action,
                            CallMenuAction::Participant(ParticipantAction::Move)
                        ) {
                            this.open_call_move_menu(&context, window, cx);
                            return;
                        }
                        this.close_layer(window, cx);
                        let participant = match &context.source {
                            Some(AccountSource::CallOccupant {
                                seat, account_id, ..
                            }) => match seat.target.place {
                                junto_client_core::protocol::call::CallPlace::Channel {
                                    channel_id,
                                    ..
                                } => Some(((seat.target.server_id, channel_id), *account_id)),
                                _ => None,
                            },
                            _ => None,
                        };
                        this.state.update(cx, |state, cx| match &control.action {
                            CallMenuAction::Microphone(action) => state.use_microphone(action),
                            CallMenuAction::Sound(off) => state.set_sound_off(*off),
                            CallMenuAction::Camera(action) => state.use_camera(action, cx),
                            CallMenuAction::Hand(raised) => state.set_hand(*raised, cx),
                            CallMenuAction::Leave => state.leave_call(cx),
                            CallMenuAction::WatchShare(id) => state.watch_share(*id),
                            CallMenuAction::StopShare(slot) => state.stop_share(Some(*slot)),
                            CallMenuAction::Participant(action) => {
                                if let Some((room, account)) = participant {
                                    match action {
                                        ParticipantAction::Invite => state.invite(account, cx),
                                        ParticipantAction::Withdraw(id) => {
                                            state.withdraw_invitation(*id, cx)
                                        }
                                        ParticipantAction::AllowSpeak => {
                                            state.allow_to_speak(account, cx)
                                        }
                                        ParticipantAction::Microphone(block) => {
                                            state.set_microphone_block(room, account, *block, cx)
                                        }
                                        ParticipantAction::Sound(block) => {
                                            state.set_sound_block(room, account, *block, cx)
                                        }
                                        ParticipantAction::LowerHand => {
                                            state.lower_hand_of(room, account, cx)
                                        }
                                        ParticipantAction::Move => {}
                                        ParticipantAction::Disconnect => {
                                            state.disconnect(room, account, cx)
                                        }
                                    }
                                }
                            }
                        });
                    }))
                    .into_any_element()
            })
            .collect()
    }
}

impl Shell {
    pub(super) fn participant_controls(
        &self,
        open: &OpenAccountMenu,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        use junto_client_core::protocol::call::CallPlace;
        let Some(AccountSource::CallOccupant {
            seat, account_id, ..
        }) = &open.source
        else {
            return Vec::new();
        };
        if *account_id == open.owner.account_id || !self.call_menu_current(open, cx) {
            return Vec::new();
        }
        let CallPlace::Channel { channel_id, .. } = seat.target.place else {
            return Vec::new();
        };
        if matches!(
            &open.source,
            Some(AccountSource::CallOccupant {
                surface: super::account_menus::CallSurface::Member { inside: false }
                    | super::account_menus::CallSurface::InviteChoice,
                ..
            })
        ) {
            let Some(channel) = self
                .view
                .as_ref()
                .and_then(|view| view.lists())
                .and_then(|lists| lists.call.as_ref())
                .and_then(|panel| panel.channel())
            else {
                return Vec::new();
            };
            let Some(member) = channel
                .outside
                .iter()
                .find(|person| person.account_id == *account_id)
            else {
                return Vec::new();
            };
            let (id, label, action) = match member.invitation {
                Some(id) => (
                    "call-menu-withdraw-invitation",
                    text::WITHDRAW_INVITATION,
                    ParticipantAction::Withdraw(id),
                ),
                None => (
                    "call-menu-invite",
                    text::INVITE_TO_MY_ROOM,
                    ParticipantAction::Invite,
                ),
            };
            return self.call_controls(
                open,
                vec![CallControl {
                    id,
                    label,
                    action: CallMenuAction::Participant(action),
                    disabled: !channel.room.ready,
                    detail: None,
                }],
                cx,
            );
        }
        let Some(menu) = self
            .view
            .as_ref()
            .and_then(|view| view.person_menu(channel_id, *account_id))
        else {
            return Vec::new();
        };
        let make = |id, label, action, detail: &str| CallControl {
            id,
            label,
            action: CallMenuAction::Participant(action),
            disabled: false,
            detail: Some(detail.to_owned()),
        };
        let mut items = Vec::new();
        if !menu.destinations.is_empty() {
            items.push(make(
                "call-menu-move",
                text::MOVE_TO,
                ParticipantAction::Move,
                text::PERMISSION_MOVE,
            ));
        }
        if menu.allow_to_speak {
            items.push(make(
                "call-menu-allow-speak",
                text::ALLOW_TO_SPEAK,
                ParticipantAction::AllowSpeak,
                &text::allow_help(menu.name.as_deref().unwrap_or(text::UNKNOWN_PERSON)),
            ));
        } else if let Some(blocked) = menu.microphone_blocked {
            items.push(make(
                "call-menu-block-microphone",
                if blocked {
                    text::REMOVE_MICROPHONE_BLOCK
                } else {
                    text::MUTE_FOR_EVERYONE
                },
                ParticipantAction::Microphone(!blocked),
                text::PERMISSION_MUTE,
            ));
        }
        if let Some(blocked) = menu.sound_blocked {
            items.push(make(
                "call-menu-block-sound",
                if blocked {
                    text::REMOVE_SOUND_BLOCK
                } else {
                    text::BLOCK_SOUND
                },
                ParticipantAction::Sound(!blocked),
                text::PERMISSION_DEAFEN,
            ));
        }
        if let Some(raised) = menu.hand_raised {
            let mut item = make(
                "call-menu-lower-hand",
                text::LOWER_HAND_OF_PERSON,
                ParticipantAction::LowerHand,
                if raised {
                    text::LOWER_HAND_HELP
                } else {
                    text::NO_RAISED_HAND
                },
            );
            item.disabled = !raised;
            items.push(item);
        }
        if menu.disconnect {
            items.push(make(
                "call-menu-disconnect",
                text::DISCONNECT,
                ParticipantAction::Disconnect,
                text::PERMISSION_DISCONNECT,
            ));
        }
        self.call_controls(open, items, cx)
    }
}

impl Shell {
    pub(super) fn call_participant_menu(
        open: &OpenAccountMenu,
        view: &crate::state::view::ShellView,
    ) -> Option<crate::state::view::PersonMenu> {
        let Some(AccountSource::CallOccupant {
            seat, account_id, ..
        }) = &open.source
        else {
            return None;
        };
        let junto_client_core::protocol::call::CallPlace::Channel { channel_id, .. } =
            seat.target.place
        else {
            return None;
        };
        view.person_menu(channel_id, *account_id)
    }

    fn open_call_move_menu(
        &mut self,
        open: &OpenAccountMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.call_menu_current(open, cx)
            || self
                .view
                .as_ref()
                .and_then(|view| Self::call_participant_menu(open, view))
                .is_none_or(|menu| menu.destinations.is_empty())
        {
            return;
        }
        self.open_layer(Layer::CallMoveMenu(Box::new(open.clone())), window, cx);
    }

    pub(super) fn render_call_move_menu(
        &mut self,
        view: &crate::state::view::ShellView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Layer::CallMoveMenu(open) = &self.layer else {
            return div().into_any_element();
        };
        let open = open.clone();
        let Some(menu) = Self::call_participant_menu(&open, view) else {
            return div().into_any_element();
        };
        self.person_menu_contents(&menu, true, window, cx)
            .popup_at_stable(open.position, self.account_menu_placement.clone())
    }
}
