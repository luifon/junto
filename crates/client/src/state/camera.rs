use super::*;

impl AppState {
    pub fn media_calls(&self) -> junto_media::calls::CallsView {
        #[cfg(feature = "test-support")]
        if let Some(media) = &self.media_fixture {
            return media.calls.clone();
        }
        self.calls.view()
    }

    /// The calls as the state of the application has taken them last. They change when
    /// the state has done its work for a change of the calls.
    pub fn shown_calls(&self) -> junto_media::calls::CallsView {
        #[cfg(feature = "test-support")]
        if let Some(media) = &self.media_fixture {
            return media.calls.clone();
        }
        self.known.media.calls.clone()
    }

    /// What the camera control does as it is drawn now: it stops a camera that starts or
    /// runs, and starts one at the seat of the call. Absent without a call.
    pub fn camera_action(&self) -> Option<CameraAction> {
        self.media_calls()
            .call
            .as_ref()
            .map(CameraAction::drawn_for)
    }

    /// Does what the camera control showed when it was drawn. A start for another call or
    /// seat than the one of the device now starts nothing (ADR 0788).
    pub fn use_camera(&mut self, action: &CameraAction, cx: &mut Context<Self>) {
        match action {
            CameraAction::Stop => self.calls.stop_camera(),
            CameraAction::Start(seat) => self
                .calls
                .start_camera_in(seat, self.known.media.voice.camera_device.clone()),
        }
        cx.notify();
    }

    pub fn choose_camera(&mut self, device: Option<String>, cx: &mut Context<Self>) {
        // A camera that runs starts again with the chosen device, at the seat where it
        // runs.
        let running = self
            .calls
            .view()
            .call
            .filter(camera_is_on)
            .map(|call| call.seat());
        self.change_voice(cx, move |voice| voice.camera_device.clone_from(&device));
        if let Some(seat) = running {
            self.calls
                .start_camera_in(&seat, self.known.media.voice.camera_device.clone());
        }
    }
    pub fn camera_choices(&self) -> Vec<view::DeviceChoice> {
        let chosen = &self.known.media.voice.camera_device;
        let mut choices = vec![view::DeviceChoice {
            id: None,
            name: String::new(),
            selected: chosen.is_none(),
        }];
        for device in &self.camera_devices {
            choices.push(view::DeviceChoice {
                selected: chosen.as_ref() == Some(&device.id),
                id: Some(device.id.clone()),
                name: device.name.clone(),
            });
        }
        if let Some(id) = chosen
            && !choices.iter().any(|choice| choice.selected)
        {
            choices.push(view::DeviceChoice {
                id: Some(id.clone()),
                name: String::new(),
                selected: true,
            });
        }
        choices
    }
}

fn camera_is_on(call: &ActiveCall) -> bool {
    matches!(
        call.camera,
        junto_media::camera::CameraState::Starting | junto_media::camera::CameraState::Running
    )
}

impl CameraAction {
    /// What the camera control of a call does: it stops a camera that starts or runs, and
    /// starts one at the seat of the call.
    pub fn drawn_for(call: &ActiveCall) -> Self {
        if camera_is_on(call) {
            Self::Stop
        } else {
            Self::Start(call.seat())
        }
    }
}
