//! Camera layout and pictures in the call window.

use junto_client_core::protocol::ids::AccountId;

pub(crate) fn visible_people(
    people: &[AccountId],
    cameras: &[AccountId],
    hide: bool,
) -> Vec<AccountId> {
    people
        .iter()
        .copied()
        .filter(|person| !hide || cameras.contains(person))
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
        CameraState::Failed(CameraError::Blocked) => crate::text::CAMERA_BLOCKED,
        CameraState::Failed(CameraError::Unsupported) => crate::text::CAMERA_UNSUPPORTED,
        CameraState::Failed(CameraError::Interrupted) => crate::text::CAMERA_INTERRUPTED,
        CameraState::Failed(CameraError::Failed(_)) => crate::text::CAMERA_FAILED,
        _ => return None,
    })
}

impl super::Shell {
    pub(crate) fn show_cameras(
        &mut self,
        frames: &std::collections::HashMap<
            junto_media::engine::ShareId,
            std::sync::Arc<junto_media::camera::CameraFrame>,
        >,
        cx: &mut gpui::Context<Self>,
    ) {
        let mut released = Vec::new();
        self.cameras.retain(|id, (old, picture)| {
            let keep = frames
                .get(id)
                .is_some_and(|frame| std::sync::Arc::ptr_eq(frame, old));
            if !keep && let Some(image) = picture.image() {
                released.push(image.clone());
            }
            keep
        });
        for (id, frame) in frames {
            if !self.cameras.contains_key(id) {
                #[cfg(target_os = "macos")]
                let surface = frame.surface().map(crate::ui::tile::CameraPicture::Surface);
                #[cfg(not(target_os = "macos"))]
                let surface: Option<crate::ui::tile::CameraPicture> = None;
                if let Some(picture) = surface.or_else(|| {
                    super::share::render_image(frame.picture()?)
                        .map(crate::ui::tile::CameraPicture::Image)
                }) {
                    self.cameras.insert(*id, (frame.clone(), picture));
                }
            }
        }
        for image in released {
            cx.drop_image(image, None);
        }
        cx.notify();
    }
}

#[cfg(feature = "test-support")]
impl super::Shell {
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
        self.camera_focus = focus;
        self.detached = detached;
        self.show_cameras(&pictures, cx);
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
    fn camera_layout_filters_tiles_without_hiding_members() {
        let one = AccountId::generate();
        let two = AccountId::generate();
        let three = AccountId::generate();
        let members = [one, two, three];
        assert_eq!(visible_people(&members, &[two], false), members);
        assert_eq!(visible_people(&members, &[two], true), [two]);
        assert_eq!(members, [one, two, three]);
        assert!(visible_people(&members, &[], true).is_empty());
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
