//! Camera layout and pictures in the call window.

use junto_client_core::protocol::ids::AccountId;

pub(crate) fn default_name() -> &'static str {
    "First available camera"
}

pub(crate) fn visible_people(
    people: &[AccountId],
    cameras: &[AccountId],
    hide: bool,
) -> Vec<AccountId> {
    people
        .iter()
        .copied()
        .filter(|person| !hide || cameras.is_empty() || cameras.contains(person))
        .collect()
}

pub(crate) fn hidden_people(
    people: &[AccountId],
    cameras: &[AccountId],
    hide: bool,
) -> Vec<AccountId> {
    people
        .iter()
        .copied()
        .filter(|person| hide && !cameras.is_empty() && !cameras.contains(person))
        .collect()
}

pub(crate) fn focused_camera(
    current: Option<AccountId>,
    cameras: &[AccountId],
    speakers: &[AccountId],
) -> Option<AccountId> {
    speakers
        .iter()
        .copied()
        .find(|id| cameras.contains(id))
        .or_else(|| current.filter(|id| cameras.contains(id)))
        .or_else(|| cameras.first().copied())
}

impl super::Shell {
    pub(crate) fn draws_call(&self, cx: &gpui::App) -> bool {
        let state = self.state.read(cx);
        let Some(active) = state.media_calls().call else {
            return false;
        };
        if active.phase != junto_media::calls::Phase::Connected {
            return false;
        }
        if self.detached {
            return true;
        }
        if state.is_call_detached() {
            return false;
        }
        self.view.as_ref().is_some_and(|view| {
            view.call.as_ref().is_some_and(|call| call.shown)
                || (view.selected.as_ref().is_some_and(|server| server.id == active.target.server_id)
                    && view.workspace().and_then(|workspace| workspace.conversation.as_ref()).is_some_and(|conversation| {
                        matches!(active.target.place, junto_client_core::protocol::call::CallPlace::Direct {low_account_id, high_account_id} if low_account_id == conversation.account_id || high_account_id == conversation.account_id)
                    }))
        })
    }

    pub(crate) fn camera_request(
        &self,
        person: &crate::state::view::Occupant,
        cx: &gpui::App,
    ) -> Option<(junto_media::calls::Calls, junto_media::engine::ShareId)> {
        use junto_media::{camera::CameraState, engine::ShareId};
        let state = self.state.read(cx);
        let call = state.media_calls().call?;
        let id = if person.is_self {
            if !matches!(call.camera, CameraState::Starting | CameraState::Running) {
                return None;
            }
            ShareId::Own
        } else {
            if !call.cameras.contains(&person.account_id) {
                return None;
            }
            ShareId::Of(person.account_id)
        };
        Some((state.calls().clone(), id))
    }

    pub(crate) fn sync_camera_visibility(&self, cx: &gpui::App) {
        use junto_media::{calls::Phase, camera::CameraState, engine::ShareId};
        let state = self.state.read(cx);
        if !self.detached && state.is_call_detached() {
            return;
        }
        let visible = self.draws_call(cx);
        let mut ids = Vec::new();
        if visible
            && let Some(call) = state.media_calls().call
            && call.phase == Phase::Connected
        {
            ids.extend(call.cameras.into_iter().map(ShareId::Of));
            if matches!(call.camera, CameraState::Starting | CameraState::Running) {
                ids.push(ShareId::Own);
            }
        }
        state.calls().retain_camera_views(&ids);
    }

    pub(crate) fn camera_image(
        &self,
        person: &crate::state::view::Occupant,
    ) -> Option<crate::ui::tile::CameraPicture> {
        let key = if person.is_self {
            junto_media::engine::ShareId::Own
        } else {
            junto_media::engine::ShareId::Of(person.account_id)
        };
        self.cameras.get(&key).map(|(_, image)| image.clone())
    }
}

pub(crate) fn failure(state: &junto_media::camera::CameraState) -> Option<&'static str> {
    use junto_media::camera::{CameraError, CameraState};
    Some(match state {
        CameraState::Failed(CameraError::NoPermission(
            junto_media::camera::CameraAccess::Restricted,
        )) => crate::text::CAMERA_RESTRICTED,
        CameraState::Failed(CameraError::NoPermission(_)) => crate::text::CAMERA_REFUSED,
        CameraState::Failed(CameraError::DeviceGone) => crate::text::CAMERA_GONE,
        CameraState::Failed(CameraError::NotConnected) => {
            "Wait for the call to connect, then turn on the camera."
        }
        CameraState::Failed(CameraError::Blocked) => crate::text::CAMERA_BLOCKED,
        CameraState::Failed(CameraError::Unsupported) => crate::text::CAMERA_UNSUPPORTED,
        CameraState::Failed(CameraError::Interrupted) => crate::text::CAMERA_INTERRUPTED,
        CameraState::Failed(CameraError::Failed(_)) => crate::text::CAMERA_FAILED,
        _ => return None,
    })
}

impl super::Shell {
    /// Holds the pictures of these cameras: a picture that the window holds is kept, and a
    /// converted one is taken. Returns whether the window holds other pictures than before,
    /// and is drawn for them.
    pub(crate) fn show_cameras(
        &mut self,
        frames: super::prepared::ConvertedCameras,
        cx: &mut gpui::Context<Self>,
    ) -> bool {
        let mut released = Vec::new();
        let mut changed = false;
        self.cameras.retain(|id, (old, picture)| {
            let keep = frames
                .get(id)
                .is_some_and(|(frame, _)| std::sync::Arc::ptr_eq(frame, old));
            if !keep && let Some(image) = picture.image() {
                released.push(image.clone());
            }
            changed |= !keep;
            keep
        });
        for (id, (frame, converted)) in frames {
            if !self.cameras.contains_key(&id)
                && let Some(picture) = converted.and_then(|converted| converted.picture(&frame))
            {
                self.cameras.insert(id, (frame, picture));
                changed = true;
            }
        }
        for image in released {
            crate::images::release(image, cx);
        }
        if changed {
            cx.notify();
        }
        changed
    }
}

#[cfg(feature = "test-support")]
impl super::Shell {
    pub fn drawn_camera_count(&self) -> usize {
        self.cameras.len()
    }

    pub fn show_cameras_fixture(
        &mut self,
        pictures: std::collections::HashMap<
            junto_media::engine::ShareId,
            std::sync::Arc<junto_media::camera::CameraFrame>,
        >,
        focus: bool,
        detached: bool,
        cx: &mut gpui::Context<Self>,
    ) {
        self.picture_tasks.clear();
        self.drawing_call = true;
        self.camera_focus = focus;
        self.detached = detached;
        let converted = pictures
            .into_iter()
            .map(|(id, frame)| {
                let picture = super::prepared::ConvertedCamera::of(&frame);
                (id, (frame, picture))
            })
            .collect();
        self.show_cameras(converted, cx);
    }
}

pub(crate) type ShownCameras = std::collections::HashMap<
    junto_media::engine::ShareId,
    (
        std::sync::Arc<junto_media::camera::CameraFrame>,
        crate::ui::tile::CameraPicture,
    ),
>;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn the_camera_default_names_the_first_available_device() {
        assert_eq!(default_name(), "First available camera");
    }

    #[test]
    fn camera_layout_filters_tiles_without_hiding_members() {
        let one = AccountId::generate();
        let two = AccountId::generate();
        let three = AccountId::generate();
        let members = [one, two, three];
        assert_eq!(visible_people(&members, &[two], false), members);
        assert_eq!(visible_people(&members, &[two], true), [two]);
        assert_eq!(members, [one, two, three]);
        assert_eq!(visible_people(&members, &[], true), members);
    }
    #[test]
    fn the_hidden_avatars_are_the_two_people_without_video() {
        let people: Vec<_> = (0..5).map(|_| AccountId::generate()).collect();
        assert_eq!(
            hidden_people(&people, &[people[0], people[2], people[3]], true),
            [people[1], people[4]]
        );
        assert!(hidden_people(&people, &[people[0]], false).is_empty());
    }

    #[test]
    fn camera_focus_follows_a_speaker_with_video_and_keeps_a_stable_fallback() {
        let one = AccountId::generate();
        let two = AccountId::generate();
        let three = AccountId::generate();
        assert_eq!(focused_camera(None, &[one, two], &[two]), Some(two));
        assert_eq!(focused_camera(Some(two), &[one, two], &[]), Some(two));
        assert_eq!(focused_camera(Some(two), &[one, two], &[one]), Some(one));
        assert_eq!(
            focused_camera(Some(three), &[one, two], &[three]),
            Some(one)
        );
        assert_eq!(focused_camera(Some(one), &[], &[one]), None);
    }
}
