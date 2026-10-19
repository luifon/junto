//! The conversion of the pictures of a call into images of the window, on a blocking
//! thread of the runtime of the client (ADR 0730, ADR 0793). The thread of the window only
//! puts the converted pictures in place of the ones that it holds.

use std::collections::HashMap;
use std::sync::Arc;

use gpui::RenderImage;
use junto_media::engine::{MainPicture, ShareId};
use junto_media::share::picture::{PictureFrame, VideoPicture};

use super::share::render_image;
use crate::ui::DrawnPicture;

/// The newest pictures of the sources that have a newer one. A source without a newer
/// picture is absent.
pub(crate) struct Wanted {
    pub main: Option<Option<MainPicture>>,
    pub small: Option<HashMap<ShareId, Arc<PictureFrame>>>,
    pub cameras: Option<HashMap<ShareId, Arc<VideoPicture>>>,
}

/// The pictures that the window holds when a conversion starts.
#[derive(Default)]
pub(crate) struct Held {
    pub main: Option<Arc<VideoPicture>>,
    pub small: HashMap<ShareId, Arc<PictureFrame>>,
    pub cameras: HashMap<ShareId, Arc<VideoPicture>>,
}

/// The watched share after a conversion: its newest picture, and the form that the window
/// draws. The form is absent when no share is watched or the picture could not be
/// converted.
pub(crate) struct ConvertedMain {
    pub frame: Option<Arc<VideoPicture>>,
    pub shown: Option<(ShareId, ConvertedPicture)>,
}

/// The small pictures of the shares after a conversion, each with its image.
pub(crate) type ConvertedSmall = HashMap<ShareId, (Arc<PictureFrame>, Option<Arc<RenderImage>>)>;

/// The camera pictures after a conversion, each in the form that the window draws.
pub(crate) type ConvertedCameras = HashMap<ShareId, (Arc<VideoPicture>, Option<ConvertedPicture>)>;

/// The pictures of the sources that had a newer one, converted. An entry without an image
/// is a picture that the window held when the conversion started; it was not converted
/// again. A picture that could not be converted is also without an image, and the window
/// does not take it.
pub(crate) struct Converted {
    pub main: Option<ConvertedMain>,
    pub small: Option<ConvertedSmall>,
    pub cameras: Option<ConvertedCameras>,
}

/// A picture of a camera or of the watched share in the form that the window draws.
pub(crate) enum ConvertedPicture {
    Image(Arc<RenderImage>),
    /// The pixel buffer of Core Video of this picture was made. The picture keeps it; the
    /// window takes it from the picture.
    #[cfg(target_os = "macos")]
    Surface(Arc<VideoPicture>),
}

impl ConvertedPicture {
    /// Converts a picture: on macOS into a pixel buffer of Core Video when the system gives
    /// one, otherwise into an image that takes the converted pixels without a copy.
    pub(crate) fn of(frame: &Arc<VideoPicture>) -> Option<Self> {
        #[cfg(target_os = "macos")]
        if frame.surface().is_some() {
            return Some(Self::Surface(frame.clone()));
        }
        render_image(frame.bgra()?).map(Self::Image)
    }

    /// The picture that the window draws. Takes no time: the conversion is done.
    pub(crate) fn picture(self) -> Option<DrawnPicture> {
        match self {
            Self::Image(image) => Some(DrawnPicture::Image(image)),
            #[cfg(target_os = "macos")]
            Self::Surface(frame) => frame.surface().map(DrawnPicture::Surface),
        }
    }
}

impl Wanted {
    /// Converts every wanted picture that the window does not hold. Copies pixels: it
    /// runs on a blocking thread of the runtime, not on the thread of the window.
    pub(crate) fn prepare(self, held: &Held) -> Converted {
        let main = self.main.and_then(|picture| {
            let frame = picture.as_ref().map(|picture| picture.frame.clone());
            let same = match (&frame, &held.main) {
                (Some(now), Some(taken)) => Arc::ptr_eq(now, taken),
                (None, None) => true,
                _ => false,
            };
            (!same).then(|| ConvertedMain {
                frame,
                shown: picture.and_then(|picture| {
                    Some((picture.share, ConvertedPicture::of(&picture.frame)?))
                }),
            })
        });
        let small = self.small.map(|pictures| {
            pictures
                .into_iter()
                .map(|(share, frame)| {
                    let kept = held
                        .small
                        .get(&share)
                        .is_some_and(|taken| Arc::ptr_eq(taken, &frame));
                    let image = if kept {
                        None
                    } else {
                        render_image(PictureFrame::clone(&frame))
                    };
                    (share, (frame, image))
                })
                .collect()
        });
        let cameras = self.cameras.map(|frames| {
            frames
                .into_iter()
                .map(|(id, frame)| {
                    let kept = held
                        .cameras
                        .get(&id)
                        .is_some_and(|taken| Arc::ptr_eq(taken, &frame));
                    let picture = if kept {
                        None
                    } else {
                        ConvertedPicture::of(&frame)
                    };
                    (id, (frame, picture))
                })
                .collect()
        });
        Converted {
            main,
            small,
            cameras,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use junto_client_core::protocol::ids::AccountId;
    use junto_media::share::synthetic::two_colours;

    fn picture(width: u32) -> Arc<PictureFrame> {
        Arc::new(PictureFrame {
            width,
            height: 2,
            bgra: vec![255; (width * 2 * 4) as usize],
            source: (width, 2),
        })
    }

    fn video(width: u32) -> Arc<VideoPicture> {
        let pixels = two_colours(width, 2, (255, 255, 255), (255, 255, 255));
        Arc::new(VideoPicture::new(&pixels, false, (width, 2)).unwrap())
    }

    #[test]
    fn a_conversion_converts_the_pictures_that_the_window_does_not_hold() {
        let (one, two) = (
            ShareId::Of(AccountId::generate()),
            ShareId::Of(AccountId::generate()),
        );
        let (held_small, new_small, watched) = (picture(2), picture(3), video(4));
        let held = Held {
            main: Some(watched.clone()),
            small: [(one, held_small.clone())].into_iter().collect(),
            cameras: HashMap::new(),
        };
        let converted = Wanted {
            main: Some(Some(MainPicture {
                share: one,
                frame: watched.clone(),
            })),
            small: Some(
                [(one, held_small.clone()), (two, new_small.clone())]
                    .into_iter()
                    .collect(),
            ),
            cameras: None,
        }
        .prepare(&held);
        // The watched picture is the one that the window holds: nothing to take.
        assert!(converted.main.is_none());
        let small = converted.small.unwrap();
        let (frame, image) = &small[&one];
        assert!(Arc::ptr_eq(frame, &held_small) && image.is_none());
        let (frame, image) = &small[&two];
        assert!(Arc::ptr_eq(frame, &new_small));
        assert_eq!(
            image.as_ref().unwrap().size(0),
            gpui::size(3.into(), 2.into())
        );
        assert!(converted.cameras.is_none());

        // A newer watched picture is converted; a share that ends leaves no image.
        let newer = video(6);
        let converted = Wanted {
            main: Some(Some(MainPicture {
                share: two,
                frame: newer.clone(),
            })),
            small: None,
            cameras: None,
        }
        .prepare(&held);
        let main = converted.main.unwrap();
        assert!(Arc::ptr_eq(main.frame.as_ref().unwrap(), &newer));
        assert_eq!(main.shown.unwrap().0, two);
        let ended = Wanted {
            main: Some(None),
            small: None,
            cameras: None,
        }
        .prepare(&held)
        .main
        .unwrap();
        assert!(ended.frame.is_none() && ended.shown.is_none());
    }

    /// The watched share is converted as a camera picture is: on macOS into the pixel
    /// buffer of Core Video that the renderer draws without a copy, and on the other
    /// systems into an image that holds the converted pixels.
    #[test]
    fn the_watched_share_is_converted_in_the_form_of_a_camera_picture() {
        let share = ShareId::Of(AccountId::generate());
        let converted = Wanted {
            main: Some(Some(MainPicture {
                share,
                frame: video(64),
            })),
            small: None,
            cameras: None,
        }
        .prepare(&Held::default());
        let (shown, picture) = converted.main.unwrap().shown.unwrap();
        assert_eq!(shown, share);
        #[cfg(target_os = "macos")]
        assert!(matches!(picture, ConvertedPicture::Surface(_)));
        #[cfg(not(target_os = "macos"))]
        assert!(matches!(picture, ConvertedPicture::Image(_)));
        assert_eq!(picture.picture().unwrap().width(), 64);
    }
}
