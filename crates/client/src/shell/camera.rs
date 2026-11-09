//! Camera layout and pictures in the call window.

use gpui::{
    Hsla, InteractiveElement, ParentElement, Role, StatefulInteractiveElement, Styled, StyledText,
    div,
};
use junto_client_core::protocol::ids::AccountId;

use crate::state::view::Occupant;
use crate::theme::{Theme, u};
use crate::ui::StyleExt;

/// The size in pixels that the device preview asks for the picture that the call sends, when
/// no tile of the own camera is drawn: the preview area of the settings at its widest.
const OWN_PREVIEW_PIXELS: (u32, u32) = (640, 360);

pub(crate) fn default_name() -> &'static str {
    "First available camera"
}

/// The people whose tiles the stage leaves out: with "Hide people without video" on, every
/// person without a camera in the call, also when nobody has one.
pub(crate) fn hidden_people(
    people: &[AccountId],
    with_video: &[AccountId],
    hide: bool,
) -> Vec<AccountId> {
    people
        .iter()
        .copied()
        .filter(|person| hide && !with_video.contains(person))
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
                || (view
                    .selected
                    .as_ref()
                    .is_some_and(|server| server.id == active.target.server_id)
                    && view
                        .lists()
                        .and_then(|lists| lists.conversation.as_ref())
                        .is_some_and(|conversation| {
                            match (conversation.account_id, active.target.place) {
                                (
                                    None,
                                    junto_client_core::protocol::call::CallPlace::Group {
                                        conversation_id,
                                    },
                                ) => conversation_id == conversation.id,
                                (
                                    Some(other),
                                    junto_client_core::protocol::call::CallPlace::Direct {
                                        low_account_id,
                                        high_account_id,
                                    },
                                ) => low_account_id == other || high_account_id == other,
                                _ => false,
                            }
                        }))
        })
    }

    /// Whether this window converts the pictures of the call: it draws the call, or it shows
    /// the devices of the person during the call, whose preview is the picture that the call
    /// sends (ADR 0735).
    pub(crate) fn wants_call_pictures(&self, cx: &gpui::App) -> bool {
        self.draws_call(cx) || (!self.detached && self.state.read(cx).call_devices_shown())
    }

    /// Whether this window shows the device component of the person while a call runs: the
    /// settings at "Voice and video", or the device check before a join.
    pub(crate) fn shows_call_devices(&self, cx: &gpui::App) -> bool {
        !self.detached
            && self.state.read(cx).media_calls().call.is_some()
            && match &self.layer {
                super::Layer::Settings => {
                    self.settings_section() == super::settings::SettingsSection::VoiceAndVideo
                }
                super::Layer::Prejoin(_) => true,
                _ => false,
            }
    }

    pub(crate) fn camera_request(
        &self,
        person: &crate::state::view::Occupant,
        cx: &gpui::App,
    ) -> Option<(junto_media::calls::Calls, junto_media::engine::Person)> {
        use junto_media::{camera::CameraState, engine::Person};
        let state = self.state.read(cx);
        let call = state.media_calls().call?;
        let id = if person.is_self {
            if !matches!(call.camera, CameraState::Starting | CameraState::Running) {
                return None;
            }
            Person::Own
        } else {
            if !call.cameras.contains(&person.account_id) {
                return None;
            }
            Person::Of(person.account_id)
        };
        Some((state.calls().clone(), id))
    }

    pub(crate) fn sync_camera_visibility(&self, cx: &gpui::App) {
        use junto_media::{calls::Phase, camera::CameraState, engine::Person};
        let state = self.state.read(cx);
        if !self.detached && state.is_call_detached() {
            return;
        }
        let visible = self.draws_call(cx);
        // The devices of the person show the picture that the call sends, also while the
        // own tile is hidden or the call is not drawn (ADR 0735).
        let devices = state.call_devices_shown();
        let mut ids = Vec::new();
        let mut preview = false;
        if let Some(call) = state.media_calls().call
            && call.phase == Phase::Connected
        {
            let own_on = matches!(call.camera, CameraState::Starting | CameraState::Running);
            if visible {
                ids.extend(call.cameras.into_iter().map(Person::Of));
            }
            if own_on && ((visible && !state.voice().hide_own_camera) || devices) {
                ids.push(Person::Own);
            }
            preview = own_on && devices;
        }
        state.calls().retain_camera_views(&ids);
        // A drawn own tile states the size of the own picture. The tiles of the stage are
        // built only in view, so the own tile can be absent although the call is drawn: the
        // devices then ask for the picture at the size of their preview.
        if preview && !state.calls().has_camera_view(Person::Own) {
            state
                .calls()
                .set_camera_view(Person::Own, OWN_PREVIEW_PIXELS);
        }
    }

    /// The people with a camera in the call: a published camera of another person, or the
    /// camera of this device while it starts or runs. A camera counts before its first
    /// picture arrives, because only a drawn tile asks the media server for the picture.
    pub(crate) fn people_with_video(&self, people: &[Occupant], cx: &gpui::App) -> Vec<AccountId> {
        people
            .iter()
            .filter(|person| self.camera_request(person, cx).is_some())
            .map(|person| person.account_id)
            .collect()
    }

    /// The statement of the people whose tiles are left out, with their first avatars.
    pub(crate) fn hidden_note(
        &self,
        people: &[Occupant],
        hidden: &[AccountId],
        theme: &Theme,
        cx: &gpui::App,
    ) -> Option<gpui::Stateful<gpui::Div>> {
        if hidden.is_empty() {
            return None;
        }
        let colors = theme.colors;
        let faces = self.shown_faces();
        let (strong, rest) = crate::text::people_hidden(hidden.len());
        let line = gpui::SharedString::from(format!("{strong}{rest}"));
        let avatars = people
            .iter()
            .filter(|person| hidden.contains(&person.account_id))
            .take(3)
            .map(|person| {
                let account_id = person.account_id;
                let child = div()
                    .debug_selector(move || format!("hidden-avatar:{account_id}"))
                    .child(super::people::avatar(&faces, account_id, 20.0).radius(2.0));
                self.call_identity_target(
                    format!("hidden-person:{account_id}").into(),
                    account_id,
                    super::channels::occupant_name(person).to_string(),
                    super::account_menus::CallSurface::Hidden,
                    child,
                    cx,
                )
            });
        let selected = line.clone();
        Some(
            div()
                .id("people-hidden")
                .role(Role::Status)
                .aria_label(line.clone())
                .debug_selector(|| "people-hidden".to_owned())
                .flex()
                .flex_none()
                .items_center()
                .gap(u(9.0))
                .typeface(theme.types.small, theme)
                .text_size(u(13.0))
                .line_height(u(18.0))
                .font_weight(gpui::FontWeight::MEDIUM)
                .ink(colors.bone_2)
                .child(div().flex().gap(u(3.0)).children(avatars))
                .child(
                    div()
                        .debug_selector(move || format!("people-hidden-text:{selected}"))
                        .child(StyledText::new(line).with_highlights([(
                            0..strong.len(),
                            gpui::HighlightStyle {
                                font_weight: Some(gpui::FontWeight::BOLD),
                                color: Some(Hsla::from(colors.bone)),
                                ..gpui::HighlightStyle::default()
                            },
                        )]))
                        .child(crate::text::LISTED_IN_MEMBERS),
                ),
        )
    }

    pub(crate) fn camera_image(
        &self,
        person: &crate::state::view::Occupant,
    ) -> Option<crate::ui::DrawnPicture> {
        let key = if person.is_self {
            junto_media::engine::Person::Own
        } else {
            junto_media::engine::Person::Of(person.account_id)
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
        CameraState::Failed(CameraError::NotConnected) => crate::text::CAMERA_NOT_CONNECTED,
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
        frames: super::prepared::ConvertedPictures<junto_media::engine::Person>,
        cx: &mut gpui::Context<Self>,
    ) -> bool {
        super::prepared::take(&mut self.cameras, frames, cx)
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
            junto_media::engine::Person,
            std::sync::Arc<junto_media::share::picture::VideoPicture>,
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
                let picture = super::prepared::ConvertedPicture::of(&frame);
                (id, (frame, picture))
            })
            .collect();
        self.show_cameras(converted, cx);
    }
}

pub(crate) type ShownCameras = super::prepared::Shown<junto_media::engine::Person>;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn the_camera_default_names_the_first_available_device() {
        assert_eq!(default_name(), "First available camera");
    }

    #[test]
    fn the_hidden_people_are_those_without_video_also_when_nobody_has_video() {
        let people: Vec<_> = (0..5).map(|_| AccountId::generate()).collect();
        assert_eq!(
            hidden_people(&people, &[people[0], people[2], people[3]], true),
            [people[1], people[4]]
        );
        assert_eq!(hidden_people(&people, &[], true), people);
        assert!(hidden_people(&people, &[people[0]], false).is_empty());
        assert!(hidden_people(&people, &[], false).is_empty());
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
