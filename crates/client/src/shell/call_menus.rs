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

impl Shell {
    pub(super) fn call_menu_current(&self, open: &OpenAccountMenu, cx: &App) -> bool {
        matches!(&self.layer, Layer::AccountMenu(current) if current.as_ref() == open)
            && open
                .people
                .first()
                .is_some_and(|person| self.account_menu_current(open.owner, person, cx))
    }

    fn call_volume_value(&self, open: &OpenAccountMenu) -> Option<u16> {
        let Some(AccountSource::CallOccupant { account_id, .. }) = &open.source else {
            return None;
        };
        if *account_id == open.owner.account_id {
            return None;
        }
        self.view
            .as_ref()?
            .lists()?
            .call
            .as_ref()?
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
                state.set_volume(context.owner.entry.server_id, account, percent, cx);
                if release {
                    state.store_volume(context.owner.entry.server_id, account, cx);
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
                .child(div().truncate().child(text::your_volume_for(name)))
                .child(
                    div().flex().w_full().child(
                        VolumeSlider::new(
                            "call-menu-volume",
                            &control.state,
                            &control.focus,
                            text::volume_for(name),
                        )
                        .selector("call-menu-volume"),
                    ),
                )
                .into_any_element(),
        )
    }
}

#[derive(Clone)]
enum OwnCallAction {
    Microphone(crate::state::view::MicrophoneAction),
    Sound(bool),
    Camera(crate::state::CameraAction),
    Hand(bool),
    Leave,
}

struct OwnControl {
    id: &'static str,
    label: &'static str,
    action: OwnCallAction,
    disabled: bool,
    detail: Option<String>,
}

impl Shell {
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
        let Some(AccountSource::CallOccupant { seat, account_id }) = &open.source else {
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
        let make = |id, label, action| OwnControl {
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
            OwnCallAction::Microphone(controls.microphone_action),
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
            OwnCallAction::Camera(camera),
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
                OwnCallAction::Sound(!controls.sound_off),
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
                OwnCallAction::Hand(!raised),
            ));
        }
        items.push(make(
            "call-menu-leave",
            text::LEAVE_CALL,
            OwnCallAction::Leave,
        ));
        items
            .into_iter()
            .map(|control| {
                let context = open.clone();
                crate::ui::MenuItem::new(control.id, control.label)
                    .selector(control.id)
                    .disabled(control.disabled)
                    .when_some(control.detail, |item, detail| item.detail(detail))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if !this.call_menu_current(&context, cx) {
                            return;
                        }
                        this.close_layer(window, cx);
                        this.state.update(cx, |state, cx| match &control.action {
                            OwnCallAction::Microphone(action) => state.use_microphone(action),
                            OwnCallAction::Sound(off) => state.set_sound_off(*off),
                            OwnCallAction::Camera(action) => state.use_camera(action, cx),
                            OwnCallAction::Hand(raised) => state.set_hand(*raised, cx),
                            OwnCallAction::Leave => state.leave_call(cx),
                        });
                    }))
                    .into_any_element()
            })
            .collect()
    }
}
