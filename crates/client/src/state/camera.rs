use super::*;

impl AppState {
    pub fn media_calls(&self) -> junto_media::calls::CallsView {
        #[cfg(feature = "test-support")]
        if let Some(media) = &self.media_fixture {
            return media.calls.clone();
        }
        self.calls.view()
    }

    pub fn toggle_camera(&mut self, cx: &mut Context<Self>) {
        if self.calls.view().call.is_some_and(|call| {
            matches!(
                call.camera,
                junto_media::camera::CameraState::Starting
                    | junto_media::camera::CameraState::Running
            )
        }) {
            self.calls.stop_camera();
        } else {
            self.calls
                .start_camera(self.known.media.voice.camera_device.clone());
        }
        cx.notify();
    }
    pub fn choose_camera(&mut self, device: Option<String>, cx: &mut Context<Self>) {
        let running = self.calls.view().call.is_some_and(|c| {
            matches!(
                c.camera,
                junto_media::camera::CameraState::Running
                    | junto_media::camera::CameraState::Starting
            )
        });
        self.change_voice(cx, move |voice| voice.camera_device.clone_from(&device));
        if running {
            self.calls
                .start_camera(self.known.media.voice.camera_device.clone());
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
