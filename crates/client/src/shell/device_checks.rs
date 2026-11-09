use std::collections::HashMap;

use gpui::{
    AnyElement, Context, Hsla, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window, anchored, deferred, div,
};
use junto_client_core::{
    Owner,
    protocol::{call::CallPlace, ids::CallInvitationId},
};
use junto_media::{
    calls::CallTarget,
    camera::CameraState,
    device_check::{AudioCheckKind, AudioCheckState},
};

use super::{Layer, Shell, call::DeviceKind, prepared};
use crate::{
    assets::Icon,
    state::AppState,
    text,
    text::device_checks as words,
    theme::{Theme, theme, u},
    ui::{
        Button, ButtonKind, Dialog, Menu, MenuItem, Meter, Notice, SelectBox, StyleExt,
        dialog_footer,
    },
};

#[derive(Default)]
pub(crate) struct DeviceUi {
    pub menu: Option<DeviceKind>,
    /// The kind of the list that a press of the pointer closed: the click that follows on
    /// the box of that list does not open it again.
    pub closed_by_press: Option<DeviceKind>,
    pub scroll: gpui::ScrollHandle,
    pub preview: prepared::Shown<()>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct Prejoin {
    owner: Owner,
    target: CallTarget,
    invitation: Option<CallInvitationId>,
}

impl Prejoin {
    pub fn current(&self, state: &AppState) -> bool {
        let Some(server) = state
            .server(self.target.server_id)
            .filter(|server| server.owner() == Some(self.owner))
        else {
            return false;
        };
        let Some(model) = server.live().view().model else {
            return false;
        };
        if let Some(id) = self.invitation {
            return model
                .incoming_invitations()
                .any(|invited| invited.id == id && invited.place == self.target.place);
        }
        match self.target.place {
            CallPlace::Channel {
                workspace_id,
                channel_id,
            } => model
                .readable_channel(channel_id)
                .is_some_and(|channel| channel.workspace_id == workspace_id),
            CallPlace::Group { conversation_id } => model
                .group_conversations
                .iter()
                .any(|group| group.id == conversation_id),
            CallPlace::Direct {
                low_account_id,
                high_account_id,
            } => model.conversations.iter().any(|pair| {
                pair.low_account_id == low_account_id && pair.high_account_id == high_account_id
            }),
        }
    }

    fn name(&self, state: &AppState) -> String {
        let model = state
            .server(self.target.server_id)
            .and_then(|server| server.live().view().model);
        match self.target.place {
            CallPlace::Channel { channel_id, .. } => model
                .and_then(|model| {
                    model
                        .readable_channel(channel_id)
                        .map(|channel| channel.name.clone())
                })
                .unwrap_or_default(),
            CallPlace::Group { conversation_id } => model
                .and_then(|model| {
                    model
                        .group_conversations
                        .iter()
                        .find(|group| group.id == conversation_id)
                        .map(|group| group.name.clone())
                })
                .unwrap_or_default(),
            CallPlace::Direct {
                low_account_id,
                high_account_id,
            } => state.group_person_name(
                self.target.server_id,
                if low_account_id == self.owner.account_id {
                    high_account_id
                } else {
                    low_account_id
                },
            ),
        }
    }
}

impl Shell {
    pub fn device_check_scroll_handle(&self) -> gpui::ScrollHandle {
        self.device_ui.scroll.clone()
    }

    pub(crate) fn device_check_button(
        &self,
        target: CallTarget,
        invitation: Option<CallInvitationId>,
        selector: &'static str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let owner = self
            .state
            .read(cx)
            .server(target.server_id)
            .and_then(|server| server.owner());
        Button::new(SharedString::from(format!(
            "{selector}:{owner:?}:{target:?}:{invitation:?}"
        )))
        .label(words::CHECK)
        .icon(Icon::Phones)
        .selector(selector)
        .disabled(owner.is_none())
        .on_click(cx.listener(move |this, _, window, cx| {
            let Some(owner) = owner else {
                return;
            };
            let prejoin = Prejoin {
                owner,
                target,
                invitation,
            };
            if !prejoin.current(this.state.read(cx)) {
                return;
            }
            this.device_ui.scroll.set_offset(gpui::Point::default());
            this.open_layer(Layer::Prejoin(Box::new(prejoin)), window, cx);
            this.state
                .update(cx, |state, cx| state.set_device_checks(true, cx));
        }))
        .into_any_element()
    }

    pub(crate) fn render_prejoin(
        &mut self,
        prejoin: Prejoin,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = theme(cx);
        let name = prejoin.name(self.state.read(cx));
        let server = self
            .state
            .read(cx)
            .server(prejoin.target.server_id)
            .map(|server| server.entry().name)
            .unwrap_or_default();
        let name = words::destination(&name, &server);
        let session = self.state.read(cx).device_check_session();
        let controls = self.render_device_checks(&theme, cx);
        let dialog = Dialog::new(words::TITLE, self.layer_focus())
            .scroll(&self.device_ui.scroll)
            .selector("prejoin-dialog")
            .on_close(Self::handler(cx, |this, window, cx| {
                this.close_layer(window, cx)
            }))
            .child(
                div()
                    .typeface(theme.types.notice, &theme)
                    .ink(theme.colors.ink)
                    .child(name),
            )
            .child(controls)
            .child(dialog_footer([
                Button::new("cancel-device-check")
                    .label(text::CANCEL)
                    .selector("cancel-device-check")
                    .on_click(cx.listener(|this, _, window, cx| this.close_layer(window, cx)))
                    .into_any_element(),
                Button::new(("prejoin-join", session.map_or(0, |session| session.key())))
                    .label(text::JOIN_CALL)
                    .kind(ButtonKind::Primary)
                    .selector("prejoin-join")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if session.is_none()
                            || this.state.read(cx).device_check_session() != session
                            || !matches!(&this.layer, Layer::Prejoin(current) if **current==prejoin)
                            || !prejoin.current(this.state.read(cx))
                        {
                            return;
                        }
                        this.close_layer(window, cx);
                        this.state.update(cx, |state, cx| {
                            let joining = match prejoin.invitation {
                                Some(id) => {
                                    Box::pin(state.calls().join_invited(prejoin.target, id))
                                        as std::pin::Pin<
                                            Box<
                                                dyn std::future::Future<
                                                        Output = Result<
                                                            (),
                                                            junto_media::calls::JoinError,
                                                        >,
                                                    > + Send,
                                            >,
                                        >
                                }
                                None => Box::pin(state.calls().join(prejoin.target)),
                            };
                            cx.spawn(async move |_, _| {
                                let _ = joining.await;
                            })
                            .detach();
                        });
                    }))
                    .into_any_element(),
            ]));
        div()
            .id(("prejoin-check", session.map_or(0, |session| session.key())))
            .absolute()
            .inset_0()
            .child(dialog)
            .into_any_element()
    }

    /// The devices of the person, in the settings at "Voice and video" and in the device check
    /// before a join: the camera, input and output lists, the input level and the camera
    /// preview (ADR 0735). A choice is a setting of the person and is used at once, also by a
    /// running call. Out of a call the tests and the preview open the devices of a check;
    /// in a call the level is that of the call's microphone and the preview shows the picture
    /// that the call sends, so that no second capture opens a device that the call holds.
    pub(crate) fn render_device_checks(
        &mut self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state = self.state.read(cx);
        let view = state.calls().device_check_view();
        let call = state.media_calls().call;
        let active = call.is_some() || state.media_calls().joining.is_some();
        let session = state.device_check_session();
        let key = session.map_or(0, |session| session.key());
        let inputs = state.audio_device_choices(true);
        let outputs = state.audio_device_choices(false);
        let cameras = state.camera_choices();
        let missing = inputs
            .iter()
            .chain(&outputs)
            .chain(&cameras)
            .any(|choice| choice.selected && choice.id.is_some() && choice.name.is_empty());
        let level = state.shown_levels().input;
        let fields = [
            (
                DeviceKind::Input,
                text::INPUT_DEVICE,
                "input-device",
                inputs,
            ),
            (
                DeviceKind::Output,
                text::OUTPUT_DEVICE,
                "output-device",
                outputs,
            ),
            (DeviceKind::Camera, text::CAMERA, "camera-device", cameras),
        ];
        let mut column = div().flex().flex_col().gap(u(10.0)).mt(u(10.0)).child(
            div()
                .typeface(theme.types.small, theme)
                .ink(theme.colors.ink_2)
                .child(if active { words::IN_CALL } else { words::LOCAL }),
        );
        for (kind, label, selector, choices) in fields {
            let shown = choices
                .iter()
                .find(|choice| choice.selected)
                .map(|choice| match &choice.id {
                    None if kind == DeviceKind::Camera => super::camera::default_name().to_owned(),
                    None => text::system_default(&choice.name),
                    Some(_) if choice.name.is_empty() => text::DEVICE_NOT_CONNECTED.to_owned(),
                    Some(_) => choice.name.clone(),
                })
                .unwrap_or_else(|| text::SYSTEM_DEFAULT.to_owned());
            let mut field = div().relative().flex_none().child(
                SelectBox::new((selector, key), label, shown)
                    .selector(selector)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        // A device choice is a setting of the person, not a check: the list
                        // opens also when no check session runs, as after a change of the
                        // account or of the call ended the session, and in a call.
                        if this.state.read(cx).device_check_session() != session {
                            return;
                        }
                        if this.device_ui.closed_by_press.take() == Some(kind) {
                            return;
                        }
                        this.state.update(cx, |state, cx| state.refresh_devices(cx));
                        this.device_ui.menu = if this.device_ui.menu == Some(kind) {
                            None
                        } else {
                            Some(kind)
                        };
                        this.layer_focus.focus(window, cx);
                        cx.notify();
                    })),
            );
            if self.device_ui.menu == Some(kind) {
                let items = choices
                    .into_iter()
                    .enumerate()
                    .map(|(index, choice)| {
                        let name: SharedString = match (&choice.id, choice.name.is_empty()) {
                            (None, _) if kind == DeviceKind::Camera => {
                                super::camera::default_name().into()
                            }
                            (None, _) => text::system_default(&choice.name).into(),
                            (Some(_), true) => text::DEVICE_NOT_CONNECTED.into(),
                            _ => choice.name.into(),
                        };
                        div()
                            .flex_none()
                            .debug_selector(move || format!("check-device-choice-{index}"))
                            .child(
                                MenuItem::new(
                                    SharedString::from(format!(
                                        "check-device:{key}:{kind:?}:{:?}",
                                        choice.id
                                    )),
                                    name,
                                )
                                .icon(if choice.selected {
                                    Icon::Check
                                } else {
                                    kind.icon()
                                })
                                .selector(format!("check-device-item-{index}"))
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        if this.state.read(cx).device_check_session() != session {
                                            return;
                                        }
                                        this.device_ui.menu = None;
                                        if kind == DeviceKind::Camera {
                                            prepared::take(
                                                &mut this.device_ui.preview,
                                                HashMap::new(),
                                                cx,
                                            );
                                        }
                                        this.state.update(cx, |state, cx| match kind {
                                            DeviceKind::Input => {
                                                state.choose_input(choice.id.clone(), cx)
                                            }
                                            DeviceKind::Output => {
                                                state.choose_output(choice.id.clone(), cx)
                                            }
                                            DeviceKind::Camera => {
                                                state.choose_camera(choice.id.clone(), cx)
                                            }
                                        });
                                        cx.notify();
                                    },
                                )),
                            )
                            .into_any_element()
                    })
                    .collect::<Vec<_>>();
                field = field.child(
                    div().absolute().top(u(34.0)).left_0().child(deferred(
                        anchored().snap_to_window().child(
                            Menu::new("check-device-menu", label, self.layer_focus())
                                .width(310.0)
                                .selector("check-device-menu")
                                .on_close(Self::handler(cx, move |this, window, cx| {
                                    this.device_ui.menu = None;
                                    this.device_ui.closed_by_press =
                                        (!window.last_input_was_keyboard()).then_some(kind);
                                    cx.notify();
                                }))
                                .children(items),
                        ),
                    )),
                );
            }
            column = column.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(u(4.0))
                    .child(
                        div()
                            .typeface(theme.types.control, theme)
                            .ink(theme.colors.ink)
                            .child(label),
                    )
                    .child(field),
            );
        }

        // The tests open devices of their own: they run only out of a call.
        let mut tests = div().flex().flex_wrap().gap(u(6.0));
        let audio_tests = [
            (
                AudioCheckKind::Microphone,
                words::MICROPHONE,
                "test-microphone",
            ),
            (AudioCheckKind::Output, words::OUTPUT, "test-output"),
        ];
        for (kind, label, selector) in audio_tests.into_iter().filter(|_| !active) {
            tests = tests.child(
                Button::new((selector, key))
                    .label(if view.audio.kind() == Some(kind) {
                        words::STOP
                    } else {
                        label
                    })
                    .selector(selector)
                    .disabled(session.is_none())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.state.read(cx).device_check_session() != session {
                            return;
                        }
                        this.state
                            .update(cx, |state, cx| state.test_audio_device(kind, cx));
                    })),
            );
        }
        let camera_on = matches!(view.camera, CameraState::Starting | CameraState::Running);
        if !active {
            tests = tests.child(
                Button::new(("preview-camera", key))
                    .label(if camera_on {
                        words::STOP_CAMERA
                    } else {
                        words::CAMERA
                    })
                    .selector("preview-camera")
                    .disabled(session.is_none())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.state.read(cx).device_check_session() != session {
                            return;
                        }
                        prepared::take(&mut this.device_ui.preview, HashMap::new(), cx);
                        this.state.update(cx, |state, cx| state.preview_camera(cx));
                    })),
            );
        }
        column = column.child(tests).child(
            div()
                .id("device-check-input-level")
                .role(gpui::Role::Meter)
                .aria_label(text::INPUT_LEVEL)
                .aria_numeric_value(level as f64)
                .aria_max_numeric_value(crate::state::view::INPUT_SEGMENTS as f64)
                .debug_selector(|| "device-check-input-level".to_owned())
                .child(Meter::new(level)),
        );
        if let Some(call) = &call {
            column = column.child(self.render_call_microphone(call, theme, cx));
        }
        column =
            column.child(
                crate::ui::notice_slot("device-check-missing-slot").children(missing.then(|| {
                    Notice::new(words::DISCONNECTED).selector("check-device-disconnected")
                })),
            );
        let status = match &view.audio {
            AudioCheckState::Starting(_) => Some(
                Notice::new(words::STARTING)
                    .selector("device-test-starting")
                    .into_any_element(),
            ),
            AudioCheckState::Running(_) => Some(
                Notice::new(words::PLAYING)
                    .selector("device-test-playing")
                    .into_any_element(),
            ),
            AudioCheckState::Failed(error) => Some(
                Notice::error(words::audio_failed(error))
                    .selector("device-test-error")
                    .into_any_element(),
            ),
            AudioCheckState::Off => None,
        };
        column = column.child(crate::ui::notice_slot("device-audio-status-slot").children(status));
        // In a call the preview is the picture that the call sends: the own camera of the
        // call, which the window asks the engine for while this component is shown
        // (`sync_camera_visibility`).
        let call_camera = call.as_ref().map(|call| call.camera.clone());
        let picture = match &call_camera {
            Some(CameraState::Running) => self
                .cameras
                .get(&junto_media::engine::Person::Own)
                .map(|(_, picture)| picture.clone()),
            Some(_) => None,
            None => (view.camera == CameraState::Running)
                .then(|| {
                    self.device_ui
                        .preview
                        .get(&())
                        .map(|(_, picture)| picture.clone())
                })
                .flatten(),
        };
        let mut preview = div()
            .relative()
            .h(u(160.0))
            .w_full()
            .bg(Hsla::from(theme.colors.limestone_lo))
            .debug_selector(|| "camera-preview-area".to_owned());
        if let Some(picture) = picture {
            preview = preview.child(
                div()
                    .size_full()
                    .debug_selector(|| "camera-preview-picture".to_owned())
                    .child(picture.element()),
            );
        } else {
            preview = preview.child(
                div()
                    .p(u(10.0))
                    .typeface(theme.types.small, theme)
                    .ink(theme.colors.ink_2)
                    .child(match &call_camera {
                        Some(CameraState::Starting | CameraState::Running) => words::STARTING,
                        Some(_) => words::CALL_CAMERA_OFF,
                        None if view.camera == CameraState::Starting => words::STARTING,
                        None => words::CAMERA_OFF,
                    }),
            );
        }
        column = column.child(preview);
        let camera_error = match call_camera.as_ref().unwrap_or(&view.camera) {
            CameraState::Failed(error) => {
                Some(Notice::error(words::camera_failed(error)).selector("camera-preview-error"))
            }
            _ => None,
        };
        column = column
            .child(crate::ui::notice_slot("device-camera-status-slot").children(camera_error));
        column
            .child(
                Button::new(("refresh-check-devices", key))
                    .label(if session.is_some() || active {
                        words::REFRESH
                    } else {
                        words::CHECK
                    })
                    .selector("refresh-check-devices")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.state.read(cx).device_check_session() != session {
                            return;
                        }
                        this.state.update(cx, |state, cx| {
                            state.set_device_checks(true, cx);
                            state.refresh_devices(cx);
                        });
                    })),
            )
            .into_any_element()
    }

    /// The state of the microphone of the running call and its permission, with "Try again"
    /// when the call cannot use the microphone.
    fn render_call_microphone(
        &mut self,
        call: &junto_media::calls::ActiveCall,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use junto_media::engine::MicrophoneState;
        use junto_media::permission::MicrophoneAccess;
        let access = call.access;
        let unusable = matches!(
            call.microphone,
            MicrophoneState::NoPermission | MicrophoneState::Failed(_)
        ) || access == MicrophoneAccess::Refused;
        let seat = call.seat();
        // The question of the system, or the page of its settings (ADR 0708).
        let permission = self.permission_button(
            "microphone-permission-action",
            crate::permissions::Permission::Microphone,
            cx,
        );
        let mut column = div()
            .id("microphone-access")
            .role(gpui::Role::Status)
            .aria_label(text::microphone_access(access))
            .debug_selector(|| "microphone-access".to_owned())
            .flex()
            .flex_col()
            .gap(u(6.0))
            .child(
                div()
                    .typeface(theme.types.small, theme)
                    .ink(theme.colors.ink_2)
                    .child(text::microphone_access(access)),
            );
        if let Some(button) = permission {
            column = column.child(div().flex().child(button));
        }
        if unusable {
            column = column.child(
                div().flex().child(
                    Button::new("retry-microphone")
                        .label(text::TRY_AGAIN)
                        .icon(Icon::Retry)
                        .selector("retry-microphone")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.state.update(cx, |state, cx| {
                                state.retry_microphone(Some(&seat), cx);
                            });
                        })),
                ),
            );
        }
        column.into_any_element()
    }

    pub(crate) fn follow_device_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.detached {
            return;
        }
        let mut pictures = self.state.read(cx).calls().preview_picture();
        let runtime = self.state.read(cx).client().runtime().handle().clone();
        let task = cx.spawn_in(window, async move |this, cx| {
            while pictures.changed().await.is_ok() {
                drop(pictures.borrow_and_update());
                let Ok((generation, wanted)) =
                    this.update(cx, |this, cx| this.state.read(cx).calls().camera_preview())
                else {
                    break;
                };
                let converted = runtime
                    .spawn_blocking(move || {
                        wanted.and_then(|frame| {
                            prepared::ConvertedPicture::of(&frame)
                                .map(|converted| (frame, Some(converted)))
                        })
                    })
                    .await;
                let Ok(converted) = converted else {
                    break;
                };
                if this
                    .update(cx, |this, cx| {
                        if this.state.read(cx).calls().camera_preview().0 != generation {
                            return;
                        }
                        let pictures: HashMap<_, _> =
                            converted.into_iter().map(|picture| ((), picture)).collect();
                        if prepared::take(&mut this.device_ui.preview, pictures, cx) {
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        self.picture_tasks.push(task);
    }
}
