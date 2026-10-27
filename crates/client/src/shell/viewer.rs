//! The viewer of the pictures of a message (ADR 0740): a layer over the window that shows one
//! picture of a message fitted to the window, with its name, its size, its download and the
//! arrows to the other pictures of the message.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    AnyElement, Context, FontWeight, Hsla, InteractiveElement, IntoElement, KeyDownEvent,
    MouseButton, ParentElement, Pixels, RenderImage, Role, SharedString, Size,
    StatefulInteractiveElement, Styled, Window, canvas, div, prelude::FluentBuilder, px, size,
};
use gpui_base::FocusTrapElement as _;
use junto_client_core::protocol::ids::{AttachmentId, MessageId, ServerId};
use junto_client_core::protocol::message::Attachment;

use super::decode::{LIST_FRAME_BYTES, Stated, VIEWER_FRAME_BYTES, plan};
use super::feed::{picture_pixels, read_picture};
use super::moving::{self, Playback};
use super::{Layer, Shell};
use crate::assets::Icon;
use crate::state::AppState;
use crate::state::feed::{Feed, FeedRow};
use crate::state::view::ShellView;
use crate::text;
use crate::theme::{REM_AT_100, Theme, theme, u};
use crate::ui::{Button, ButtonKind, StyleExt, float_shadow, icon, paint_text};

/// The space beside the picture, which holds the arrows, in pixels of the design.
const SIDE_SPACE: f32 = 64.0;
/// The space under the picture, in pixels of the design.
const BOTTOM_SPACE: f32 = 24.0;
/// The height of the bar above the picture, in pixels of the design.
const BAR_HEIGHT: f32 = 52.0;
/// The longest side of a picture that the viewer reads, in pixels: a GPU draws textures of
/// at least this size.
const LONGEST_SIDE: u32 = 8192;

/// The pictures of one message in the viewer.
pub(crate) struct PictureViewer {
    pub(crate) server_id: ServerId,
    pub(crate) message_id: MessageId,
    /// The pictures of the message, in the order of its files.
    pictures: Vec<Attachment>,
    shown: usize,
    /// The shown picture at the size of the place of the viewer, read for the viewer when
    /// the message lists hold a smaller copy of it. It is released from the GPU when the
    /// viewer shows another picture or closes (ADR 0774).
    large: Option<Large>,
    /// The size of the place of the picture at the last layout, in pixels of the window.
    area: Rc<Cell<Option<Size<Pixels>>>>,
    /// The size of that place in pixels of the device, at most [`LONGEST_SIDE`].
    fit: (u32, u32),
}

struct Large {
    attachment_id: AttachmentId,
    /// The number of the read among the reads of every viewer of the window.
    number: u64,
    /// The size in pixels of the device that the read asked for.
    fit: (u32, u32),
    image: LargeImage,
}

enum LargeImage {
    Reading,
    Ready(Arc<RenderImage>),
    /// The read failed: the viewer shows the copy of the lists.
    Failed,
}

impl PictureViewer {
    fn shown(&self) -> Option<&Attachment> {
        self.pictures.get(self.shown)
    }

    /// Takes the image that the viewer read for itself, for its release.
    fn take_large(&mut self) -> Option<Arc<RenderImage>> {
        match self.large.take()?.image {
            LargeImage::Ready(image) => Some(image),
            LargeImage::Reading | LargeImage::Failed => None,
        }
    }

    /// The server left the server book, or a list of the window shows the message without
    /// the shown file: the message was deleted, or the retention removed its files. The
    /// call window lists no servers, so the server book is read in the state.
    pub(super) fn is_stale(&self, view: &ShellView, state: &AppState) -> bool {
        if state.server(self.server_id).is_none() {
            return true;
        }
        let Some(shown) = self.shown().map(|attachment| attachment.id) else {
            return true;
        };
        let lost = |feed: Option<&Feed>| {
            feed.is_some_and(|feed| {
                feed.rows.iter().any(|row| {
                    matches!(row, FeedRow::Message(row)
                        if row.message.id == self.message_id
                            && (row.message.deleted
                                || !row.message.attachments.iter().any(|file| file.id == shown)))
                })
            })
        };
        let thread = view
            .workspace()
            .and_then(|workspace| workspace.thread.as_ref());
        lost(view.lists().and_then(|lists| lists.feed.as_ref()))
            || lost(thread.and_then(|thread| thread.feed.as_ref()))
    }
}

/// True when the message lists hold the picture reduced to `fit`, or with fewer frames than
/// its file.
fn reduced_in_lists(attachment: &Attachment, fit: (u32, u32)) -> bool {
    Stated::of(attachment)
        .is_some_and(|stated| !plan(stated, Some(fit), LIST_FRAME_BYTES).whole(stated))
}

/// The place of the picture in pixels of the window: its last layout, or before it the size
/// of the window less the bar, the arrows and the space under the picture.
fn place(window: &Window, area: Option<Size<Pixels>>) -> Size<Pixels> {
    let per_design = f32::from(window.rem_size()) / REM_AT_100;
    area.unwrap_or_else(|| {
        let viewport = window.viewport_size();
        size(
            viewport.width - px(2.0 * SIDE_SPACE * per_design),
            viewport.height - px((BAR_HEIGHT + BOTTOM_SPACE + 32.0) * per_design),
        )
    })
}

/// The size of the place of the picture in pixels of the device, at most [`LONGEST_SIDE`].
fn device_fit(window: &Window, place: Size<Pixels>) -> (u32, u32) {
    let scale = window.scale_factor();
    let side =
        |length: Pixels| ((f32::from(length) * scale).round().max(1.0) as u32).min(LONGEST_SIDE);
    (side(place.width), side(place.height))
}

impl Shell {
    /// Opens the viewer on the picture at `shown` among the pictures of a message.
    pub fn open_picture_viewer(
        &mut self,
        server_id: ServerId,
        message_id: MessageId,
        pictures: Vec<Attachment>,
        shown: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(last) = pictures.len().checked_sub(1) else {
            return;
        };
        let viewer = PictureViewer {
            server_id,
            message_id,
            pictures,
            shown: shown.min(last),
            large: None,
            area: Rc::new(Cell::new(None)),
            fit: device_fit(window, place(window, None)),
        };
        self.open_layer(Layer::Picture(Box::new(viewer)), window, cx);
        self.read_viewer_picture(cx);
    }

    /// Shows the picture `step` places after the shown one. After the last picture comes the
    /// first, and before the first the last.
    pub(crate) fn step_picture_viewer(&mut self, step: isize, cx: &mut Context<Self>) {
        let Layer::Picture(viewer) = &mut self.layer else {
            return;
        };
        let count = viewer.pictures.len();
        if count < 2 {
            return;
        }
        viewer.shown = (viewer.shown as isize + step).rem_euclid(count as isize) as usize;
        if let Some(image) = viewer.take_large() {
            crate::images::release(image, cx);
        }
        self.read_viewer_picture(cx);
        cx.notify();
    }

    /// Releases the image that the viewer read for itself. The owner of the layer calls it
    /// before the layer is closed or replaced.
    pub(super) fn release_viewer_picture(&mut self, cx: &mut Context<Self>) {
        if let Layer::Picture(viewer) = &mut self.layer
            && let Some(image) = viewer.take_large()
        {
            crate::images::release(image, cx);
        }
    }

    /// Reads the shown picture at the size of the place of the viewer, and every frame of an
    /// animation within the bytes of the viewer, when the lists hold a reduced copy of it.
    /// An answer for a picture or a read that the viewer no longer shows is released at
    /// once: the number of a read counts the reads of every viewer of the window.
    fn read_viewer_picture(&mut self, cx: &mut Context<Self>) {
        let Layer::Picture(viewer) = &self.layer else {
            return;
        };
        let Some(attachment) = viewer.shown().cloned() else {
            return;
        };
        let (server_id, fit) = (viewer.server_id, viewer.fit);
        if !reduced_in_lists(&attachment, self.message_pictures.fit)
            || self.picture_of_screen_tool(attachment.id)
        {
            return;
        }
        let Some(owner) = self.state.read(cx).owner(server_id) else {
            return;
        };
        let reading = read_picture(self, owner, &attachment, Some(fit), VIEWER_FRAME_BYTES, cx);
        self.viewer_reads += 1;
        let number = self.viewer_reads;
        let Layer::Picture(viewer) = &mut self.layer else {
            return;
        };
        // A read of a larger place replaces the copy that the viewer shows when it ends.
        let shown = match viewer.large.take() {
            Some(Large {
                attachment_id,
                image: LargeImage::Ready(image),
                ..
            }) if attachment_id == attachment.id => LargeImage::Ready(image),
            Some(earlier) => {
                if let LargeImage::Ready(image) = earlier.image {
                    crate::images::release(image, cx);
                }
                LargeImage::Reading
            }
            None => LargeImage::Reading,
        };
        viewer.large = Some(Large {
            attachment_id: attachment.id,
            number,
            fit,
            image: shown,
        });
        let attachment_id = attachment.id;
        cx.spawn(async move |this, cx| {
            let decoded = reading.await.ok().flatten();
            let _ = this.update(cx, |this, cx| {
                let unused = match &mut this.layer {
                    Layer::Picture(viewer) => match &mut viewer.large {
                        Some(large)
                            if large.number == number && large.attachment_id == attachment_id =>
                        {
                            let earlier = std::mem::replace(
                                &mut large.image,
                                match decoded {
                                    Some(image) => LargeImage::Ready(image),
                                    None => LargeImage::Failed,
                                },
                            );
                            match earlier {
                                LargeImage::Ready(image) => Some(image),
                                LargeImage::Reading | LargeImage::Failed => None,
                            }
                        }
                        _ => decoded,
                    },
                    _ => decoded,
                };
                if let Some(image) = unused {
                    crate::images::release(image, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The file and the number of the last read of the viewer. A test uses it.
    #[cfg(feature = "test-support")]
    pub fn viewer_read(&self) -> Option<(AttachmentId, u64)> {
        let Layer::Picture(viewer) = &self.layer else {
            return None;
        };
        viewer
            .large
            .as_ref()
            .map(|large| (large.attachment_id, large.number))
    }

    /// The size of the image that the viewer shows, in its pixels. A test uses it.
    #[cfg(feature = "test-support")]
    pub fn viewer_image_size(&self) -> Option<(u32, u32)> {
        let Layer::Picture(viewer) = &self.layer else {
            return None;
        };
        let image = match viewer.large.as_ref().map(|large| &large.image) {
            Some(LargeImage::Ready(image)) => Some(image.clone()),
            _ => None,
        }?;
        let drawn = image.size(0);
        Some((drawn.width.0 as u32, drawn.height.0 as u32))
    }

    pub(crate) fn render_picture_viewer(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Layer::Picture(viewer) = &self.layer else {
            return div().into_any_element();
        };
        let Some(attachment) = viewer.shown().cloned() else {
            return div().into_any_element();
        };
        // A place larger than the one of the last read reads the picture again at its size.
        let fit = device_fit(window, place(window, viewer.area.get()));
        let larger =
            |read: (u32, u32)| fit.0 > read.0 + read.0 / 20 || fit.1 > read.1 + read.1 / 20;
        let read_again = viewer
            .large
            .as_ref()
            .is_some_and(|large| large.attachment_id == attachment.id && larger(large.fit));
        if let Layer::Picture(viewer) = &mut self.layer {
            viewer.fit = fit;
        }
        if read_again {
            self.read_viewer_picture(cx);
        }
        let Layer::Picture(viewer) = &self.layer else {
            return div().into_any_element();
        };
        let (server_id, shown, count) = (viewer.server_id, viewer.shown, viewer.pictures.len());
        let area = Rc::clone(&viewer.area);
        let large = match viewer.large.as_ref() {
            Some(large) if large.attachment_id == attachment.id => match &large.image {
                LargeImage::Ready(image) => Some(image.clone()),
                LargeImage::Reading | LargeImage::Failed => None,
            },
            _ => None,
        };
        let theme = theme(cx);
        let (listed, failed) = self.listed_picture(server_id, &attachment, cx);
        let image = large.or(listed);
        let bar = self.render_viewer_bar(&attachment, shown, count, &theme, window, cx);
        let playback = Rc::clone(&self.message_pictures.playback);
        let picture = Self::render_viewer_picture(
            &attachment,
            image,
            failed,
            area,
            &playback,
            &theme,
            window,
        );
        let arrows = (count > 1).then(|| {
            [
                (
                    "viewer-previous",
                    Icon::ChevronLeft,
                    text::PREVIOUS_PICTURE,
                    -1,
                ),
                ("viewer-next", Icon::ChevronRight, text::NEXT_PICTURE, 1),
            ]
            .map(|(id, shape, label, step)| {
                let strip = div().absolute().top_0().bottom_0().flex().items_center();
                let strip = if step < 0 {
                    strip.left(u(12.0))
                } else {
                    strip.right(u(12.0))
                };
                strip.child(
                    div()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            Button::new(id)
                                .icon(shape)
                                .tooltip(label)
                                .selector(id)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.step_picture_viewer(step, cx);
                                })),
                        ),
                )
            })
        });
        let label: SharedString =
            format!("{}: {}", text::PICTURE, text::label(&attachment.name)).into();
        let mut cover = Hsla::from(theme.colors.basalt_deep);
        cover.a = 0.92;
        div()
            .id("picture-viewer")
            .role(Role::Dialog)
            .aria_label(label)
            .debug_selector(|| "picture-viewer".to_owned())
            .absolute()
            .inset_0()
            .occlude()
            .child(crate::ui::cover())
            .flex()
            .flex_col()
            .bg(cover)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| this.close_layer(window, cx)),
            )
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                match event.keystroke.key.as_str() {
                    "escape" => this.close_layer(window, cx),
                    "left" => this.step_picture_viewer(-1, cx),
                    "right" => this.step_picture_viewer(1, cx),
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .child(bar)
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .px(u(SIDE_SPACE))
                    .pb(u(BOTTOM_SPACE))
                    .child(picture)
                    .children(arrows.into_iter().flatten()),
            )
            .children(super::title_bar::layer_band(window, cx))
            .focus_trap("picture-viewer-trap", self.layer_focus())
            .into_any_element()
    }

    /// The bar above the picture: the name and the size of the file, the place of the
    /// picture among the pictures of the message, the download and the close button.
    fn render_viewer_bar(
        &mut self,
        attachment: &Attachment,
        shown: usize,
        count: usize,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = theme.colors;
        let server_id = match &self.layer {
            Layer::Picture(viewer) => viewer.server_id,
            _ => return div().into_any_element(),
        };
        let download = self.download_control(server_id, attachment, shown, "viewer-", theme, cx);
        let name: SharedString = text::label(&attachment.name).into();
        let file_size: SharedString = text::file_size(attachment.bytes).into();
        let top = super::title_bar::cover_top(window, cx);
        div()
            .flex_none()
            .pt(u(top))
            .px(u(16.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(u(10.0))
                    .h(u(BAR_HEIGHT))
                    .px(u(12.0))
                    .rounded(u(theme.metrics.radius_dialog))
                    .border_1()
                    .border_color(Hsla::from(colors.rule))
                    .bg(Hsla::from(colors.travertine))
                    .shadow(float_shadow(colors.shadow_deep, 14.0, 40.0))
                    // A press on the bar does not close the viewer.
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(icon(Icon::Picture, 16.0, colors.ink_2))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .truncate()
                                    .text_size(u(14.0))
                                    .font_weight(FontWeight::BOLD)
                                    .ink(colors.ink)
                                    .debug_selector(|| "viewer-name".to_owned())
                                    .child(paint_text(name, None)),
                            )
                            .child(
                                div()
                                    .text_size(u(12.5))
                                    .ink(colors.ink_3)
                                    .debug_selector(|| "viewer-size".to_owned())
                                    .child(paint_text(file_size, None)),
                            ),
                    )
                    .when(count > 1, |bar| {
                        bar.child(
                            div()
                                .flex_none()
                                .text_size(u(13.0))
                                .ink(colors.ink_2)
                                .debug_selector(|| "viewer-position".to_owned())
                                .child(paint_text(
                                    text::picture_position(shown + 1, count).into(),
                                    None,
                                )),
                        )
                    })
                    .child(download)
                    .child(
                        Button::new("close-picture")
                            .kind(ButtonKind::Ghost)
                            .icon(Icon::X)
                            .tooltip(text::CLOSE_PICTURE)
                            .selector("close-picture")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.close_layer(window, cx)),
                            ),
                    ),
            )
            .into_any_element()
    }

    /// The picture, fitted to its place with its proportions, and never larger than its own
    /// size times the interface scale. The place is measured at each layout; a change of its
    /// size draws the window again.
    fn render_viewer_picture(
        attachment: &Attachment,
        image: Option<Arc<RenderImage>>,
        failed: bool,
        area: Rc<Cell<Option<Size<Pixels>>>>,
        playback: &Rc<Playback>,
        theme: &Theme,
        window: &Window,
    ) -> AnyElement {
        let colors = theme.colors;
        let per_design = f32::from(window.rem_size()) / REM_AT_100;
        let (width, height) = picture_pixels(attachment).unwrap_or((1, 1));
        let largest = size(
            px(width.max(1) as f32 * per_design),
            px(height.max(1) as f32 * per_design),
        );
        // Before the first layout the place is taken from the size of the window.
        let place = place(window, area.get());
        let fit = (place.width / largest.width)
            .min(place.height / largest.height)
            .clamp(0.0, 1.0);
        let drawn = size(largest.width * fit, largest.height * fit);
        let shown: AnyElement = match image {
            Some(image) => {
                moving::picture(image, format!("viewer:{}", attachment.id).into(), playback)
            }
            None => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .p(u(8.0))
                .typeface(theme.types.small, theme)
                .ink(colors.bone_2)
                .child(if failed {
                    text::PICTURE_FAILED
                } else {
                    text::PICTURE_LOADING
                })
                .into_any_element(),
        };
        div()
            .relative()
            .flex()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .items_center()
            .justify_center()
            .child(
                canvas(
                    move |bounds, window, cx| {
                        if area.get() != Some(bounds.size) {
                            area.set(Some(bounds.size));
                            if bounds.size != place {
                                window.defer(cx, |window, _| window.refresh());
                            }
                        }
                    },
                    |_, (), _, _| {},
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
            .child(
                div()
                    .id("viewer-picture")
                    .role(Role::Image)
                    .aria_label(SharedString::from(text::label(&attachment.name)))
                    .debug_selector(|| "viewer-picture".to_owned())
                    .flex_none()
                    .w(drawn.width)
                    .h(drawn.height)
                    // A press on the picture does not close the viewer.
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(shown),
            )
            .into_any_element()
    }
}
