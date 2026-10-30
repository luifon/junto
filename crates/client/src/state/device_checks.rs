use super::*;
use junto_media::device_check::AudioCheckKind;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct CheckSession {
    owner: Option<junto_client_core::Owner>,
    number: u64,
}

impl CheckSession {
    pub(crate) fn key(self) -> u64 {
        self.number
    }
}

impl AppState {
    fn check_owner(&self) -> Option<junto_client_core::Owner> {
        self.selection
            .server
            .and_then(|id| self.client.server(id))
            .and_then(|server| server.owner())
    }

    pub(crate) fn device_check_session(&self) -> Option<CheckSession> {
        self.device_checks
    }

    pub fn device_checks_open(&self) -> bool {
        self.device_checks.is_some()
    }

    pub fn set_device_checks(&mut self, on: bool, cx: &mut Context<Self>) {
        if on {
            let owner = self.check_owner();
            if self
                .device_checks
                .is_some_and(|session| session.owner == owner)
            {
                return;
            }
            self.calls.stop_device_checks();
            self.device_check_call = self.calls.view().call.map(|call| call.seat());
            self.device_check_sequence += 1;
            self.device_checks = Some(CheckSession {
                owner,
                number: self.device_check_sequence,
            });
            self.set_input_test(true);
            self.refresh_devices(cx);
        } else {
            self.device_checks = None;
            self.device_check_call = None;
            self.calls.stop_device_checks();
            self.set_input_test(false);
        }
        cx.notify();
    }

    pub(super) fn reconcile_device_checks(&mut self, cx: &mut Context<Self>) {
        let media = self.calls.view();
        if self
            .device_checks
            .is_some_and(|session| session.owner != self.check_owner())
            || (self.device_checks.is_some()
                && (media.call.as_ref().map(|call| call.seat()) != self.device_check_call
                    || media.joining.is_some()))
        {
            self.set_device_checks(false, cx);
        }
    }

    pub fn test_audio_device(&mut self, kind: AudioCheckKind, cx: &mut Context<Self>) {
        if self.calls.device_check_view().audio.kind() == Some(kind) {
            self.calls.stop_audio_check();
            self.set_input_test(self.device_checks_open());
        } else {
            self.set_device_checks(true, cx);
            self.calls.start_audio_check(kind);
        }
        cx.notify();
    }

    pub fn preview_camera(&mut self, cx: &mut Context<Self>) {
        if matches!(
            self.calls.device_check_view().camera,
            junto_media::camera::CameraState::Starting | junto_media::camera::CameraState::Running
        ) {
            self.calls.stop_camera_preview();
        } else {
            self.set_device_checks(true, cx);
            self.calls
                .start_camera_preview(self.known.media.voice.camera_device.clone());
        }
        cx.notify();
    }

    pub fn audio_device_choices(&self, input: bool) -> Vec<view::DeviceChoice> {
        let (devices, chosen) = if input {
            (
                &self.known.media.devices.inputs,
                self.voice().input_device.as_deref(),
            )
        } else {
            (
                &self.known.media.devices.outputs,
                self.voice().output_device.as_deref(),
            )
        };
        view::device_choices(devices, chosen)
    }
}
