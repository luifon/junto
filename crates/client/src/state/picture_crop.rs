//! The cut of a picture that a person chose, before it is stored (ADR 0119): the person
//! moves and zooms the square that the picture keeps, as in Discord, and the window scales
//! the square to what a server stores.

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{Context, RenderImage};
use junto_client_core::profile::{self, CHOSEN_PICTURE_MAX_BYTES};
use junto_client_core::protocol::ids::ServerId;

use super::AppState;
use crate::picture::{self, Chosen, Crop, Refusal};

/// The largest zoom of the cut: the square has a quarter of the shorter side.
pub const MOST_ZOOM: f32 = 4.0;

/// What a chosen picture is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PicturePurpose {
    /// The picture of the account on the server of the open account page.
    Account,
    /// The picture that a new account on a server starts with (ADR 0717).
    Preset,
    /// The logo of a server (ADR 0120).
    Logo(ServerId),
}

/// The cut that is open.
pub(super) struct PictureCrop {
    purpose: PicturePurpose,
    chosen: Arc<Chosen>,
    shown: Arc<RenderImage>,
    zoom: f32,
    centre: (f32, f32),
    saving: bool,
}

/// The cut as the window draws it.
#[derive(Debug, Clone)]
pub struct PictureCropView {
    pub purpose: PicturePurpose,
    pub width: u32,
    pub height: u32,
    /// From 1, the largest square, to [`MOST_ZOOM`].
    pub zoom: f32,
    /// The square that the picture keeps, in pixels of the chosen picture.
    pub crop: Crop,
    pub saving: bool,
    pub image: Arc<RenderImage>,
}

/// The chosen picture in the order of colours of the window.
fn shown_of(chosen: &Chosen) -> Arc<RenderImage> {
    let mut pixels = chosen.image.clone();
    for pixel in pixels.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Arc::new(RenderImage::new(vec![image::Frame::new(pixels)]))
}

impl AppState {
    /// Reads and decodes the file at `path` on a background thread, and opens the cut. A
    /// file that is not a picture is refused at the control of its purpose, and nothing is
    /// sent.
    pub fn choose_picture(
        &mut self,
        purpose: PicturePurpose,
        path: PathBuf,
        cx: &mut Context<Self>,
    ) {
        if self.picture_crop.is_some() {
            return;
        }
        let reading = cx.background_executor().spawn(async move {
            let bytes = profile::read_chosen_file(&path, CHOSEN_PICTURE_MAX_BYTES)
                .map_err(|_| Refusal::Unreadable)?;
            let chosen = picture::decode(&bytes)?;
            let shown = shown_of(&chosen);
            Ok::<_, Refusal>((chosen, shown))
        });
        cx.spawn(async move |this, cx| {
            let read = reading.await;
            let _ = this.update(cx, |this, cx| {
                match read {
                    Ok((chosen, shown)) => {
                        let centre = (chosen.width() as f32 / 2.0, chosen.height() as f32 / 2.0);
                        this.picture_crop = Some(PictureCrop {
                            purpose,
                            chosen: Arc::new(chosen),
                            shown,
                            zoom: 1.0,
                            centre,
                            saving: false,
                        });
                    }
                    Err(refusal) => {
                        this.picture_refused(purpose, crate::text::picture_refusal(refusal), cx)
                    }
                }
                this.changed(cx);
            });
        })
        .detach();
    }

    /// Opens the cut of these bytes at once, as a chosen file would. The screen tool uses it.
    #[cfg(feature = "test-support")]
    pub fn show_picture_crop_fixture(
        &mut self,
        purpose: PicturePurpose,
        bytes: &[u8],
        zoom: f32,
        cx: &mut Context<Self>,
    ) {
        if let Ok(chosen) = picture::decode(bytes) {
            let centre = (chosen.width() as f32 / 2.0, chosen.height() as f32 / 2.0);
            let shown = shown_of(&chosen);
            self.picture_crop = Some(PictureCrop {
                purpose,
                chosen: Arc::new(chosen),
                shown,
                zoom: zoom.clamp(1.0, MOST_ZOOM),
                centre,
                saving: false,
            });
            cx.notify();
        }
    }

    /// The cut that is open.
    pub fn picture_crop(&self) -> Option<PictureCropView> {
        let crop = self.picture_crop.as_ref()?;
        let (width, height) = (crop.chosen.width(), crop.chosen.height());
        Some(PictureCropView {
            purpose: crop.purpose,
            width,
            height,
            zoom: crop.zoom,
            crop: Crop::around(width, height, crop.zoom, crop.centre),
            saving: crop.saving,
            image: crop.shown.clone(),
        })
    }

    /// Sets the zoom of the cut, from 1 to [`MOST_ZOOM`].
    pub fn zoom_picture(&mut self, zoom: f32, cx: &mut Context<Self>) {
        if let Some(crop) = self.picture_crop.as_mut().filter(|crop| !crop.saving) {
            crop.zoom = zoom.clamp(1.0, MOST_ZOOM);
            crop.centre = Crop::around(
                crop.chosen.width(),
                crop.chosen.height(),
                crop.zoom,
                crop.centre,
            )
            .centre();
            cx.notify();
        }
    }

    /// Moves the square of the cut by `by` pixels of the chosen picture; the square stays
    /// inside the picture.
    pub fn move_picture(&mut self, by: (f32, f32), cx: &mut Context<Self>) {
        if let Some(crop) = self.picture_crop.as_mut().filter(|crop| !crop.saving) {
            let wanted = (crop.centre.0 + by.0, crop.centre.1 + by.1);
            crop.centre = Crop::around(
                crop.chosen.width(),
                crop.chosen.height(),
                crop.zoom,
                wanted,
            )
            .centre();
            cx.notify();
        }
    }

    /// Closes the cut without storing anything.
    pub fn cancel_picture(&mut self, cx: &mut Context<Self>) {
        if let Some(crop) = self.picture_crop.take() {
            crate::images::release(crop.shown, cx);
        }
        cx.notify();
    }

    /// Scales the square of the cut on a background thread and sends it as its purpose
    /// states; the cut closes.
    pub fn save_picture(&mut self, cx: &mut Context<Self>) {
        let Some(view) = self.picture_crop() else {
            return;
        };
        let Some(crop) = self.picture_crop.as_mut().filter(|crop| !crop.saving) else {
            return;
        };
        crop.saving = true;
        let chosen = crop.chosen.clone();
        let encoding = cx
            .background_executor()
            .spawn(async move { picture::encode(&chosen, view.crop) });
        cx.spawn(async move |this, cx| {
            let encoded = encoding.await;
            let _ = this.update(cx, |this, cx| {
                let purpose = view.purpose;
                if let Some(crop) = this.picture_crop.take() {
                    crate::images::release(crop.shown, cx);
                }
                match encoded {
                    Ok(bytes) => match purpose {
                        PicturePurpose::Account => this.send_account_picture(bytes, cx),
                        PicturePurpose::Preset => this.store_preset_picture(bytes, cx),
                        PicturePurpose::Logo(server_id) => {
                            this.send_server_logo(server_id, bytes, cx)
                        }
                    },
                    Err(refusal) => {
                        this.picture_refused(purpose, crate::text::picture_refusal(refusal), cx)
                    }
                }
                this.changed(cx);
            });
        })
        .detach();
        cx.notify();
    }

    /// A chosen file that was not stored: its reason is shown at the control of its
    /// purpose.
    fn picture_refused(
        &mut self,
        purpose: PicturePurpose,
        reason: &'static str,
        cx: &mut Context<Self>,
    ) {
        match purpose {
            PicturePurpose::Account => self.account_picture_refused(reason),
            PicturePurpose::Preset => self.preset_picture_refused(reason),
            PicturePurpose::Logo(_) => self.server_logo_refused(reason),
        }
        cx.notify();
    }
}
