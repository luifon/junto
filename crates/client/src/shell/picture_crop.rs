//! The cut of a chosen picture (ADR 0119): the picture in a square frame, which the person
//! drags to place it and zooms with the slider or the wheel, as in Discord. The state holds
//! the cut; the dialog lies over every layer, so that the page that chose the picture stays
//! open under it.

use gpui::{
    AnyElement, AppContext, Context, Hsla, InteractiveElement, IntoElement, MouseButton,
    MouseDownEvent, MouseMoveEvent, ParentElement, Point, ScrollWheelEvent, StatefulInteractiveElement,
    Styled, Window, div,
};
use gpui_base::slider::SliderEvent;

use super::Shell;
use super::call::VolumeControl;
use crate::state::picture_crop::{MOST_ZOOM, PictureCropView, PicturePurpose};
use crate::text;
use crate::theme::{REM_AT_100, theme, u};
use crate::ui::{Button, ButtonKind, Dialog, LevelSlider, StyleExt, dialog_footer};

/// The side of the frame of the cut, in design pixels.
const FRAME: f32 = 288.0;
/// The zoom on the slider is in percent: 100 to 400.
const ZOOM_PERCENT: (f32, f32) = (100.0, MOST_ZOOM * 100.0);

impl Shell {
    /// The state of the zoom slider, made at the first drawing of a cut.
    fn picture_zoom_control(
        &mut self,
        zoom: f32,
        cx: &mut Context<Self>,
    ) -> (gpui::Entity<gpui_base::slider::SliderState>, gpui::FocusHandle) {
        let control = self.picture_zoom.get_or_insert_with(|| {
            let state = cx.new(|_| LevelSlider::state(ZOOM_PERCENT, zoom * 100.0));
            let changes = cx.subscribe(&state, |this, _, event: &SliderEvent, cx| {
                let value = match event {
                    SliderEvent::Change(value) | SliderEvent::Release(value) => value.end(),
                };
                this.state
                    .update(cx, |state, cx| state.zoom_picture(value / 100.0, cx));
            });
            VolumeControl {
                state,
                focus: cx.focus_handle(),
                _changes: changes,
            }
        });
        (control.state.clone(), control.focus.clone())
    }

    pub(super) fn render_picture_crop(
        &mut self,
        crop: PictureCropView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = theme(cx);
        let colors = &theme.colors;
        let title = match crop.purpose {
            PicturePurpose::Account => text::PICTURE_CROP_ACCOUNT,
            PicturePurpose::Preset => text::PICTURE_CROP_PRESET,
            PicturePurpose::Logo(_) => text::PICTURE_CROP_LOGO,
        };
        // The frame shows the square: one picture pixel is `scale` design pixels.
        let scale = FRAME / crop.crop.side;
        let design_per_window_px = REM_AT_100 / f32::from(window.rem_size());
        let (zoom_state, zoom_focus) = self.picture_zoom_control(crop.zoom, cx);
        let shown = zoom_state.read(cx).value().end() / 100.0;
        if (shown - crop.zoom).abs() > 0.005 {
            let zoom = crop.zoom * 100.0;
            zoom_state.update(cx, |slider, cx| slider.set_value(zoom, window, cx));
        }
        let saving = crop.saving;
        let frame = div()
            .id("picture-crop-frame")
            .debug_selector(|| "picture-crop-frame".to_owned())
            .relative()
            .flex_none()
            .w(u(FRAME))
            .h(u(FRAME))
            .mx_auto()
            .overflow_hidden()
            .rounded(u(theme.metrics.radius))
            .bg(Hsla::from(colors.basalt_2))
            .cursor_grab()
            .child(
                gpui::img(crop.image.clone())
                    .absolute()
                    .left(u(-crop.crop.left * scale))
                    .top(u(-crop.crop.top * scale))
                    .w(u(crop.width as f32 * scale))
                    .h(u(crop.height as f32 * scale)),
            )
            // The round mark shows what a picture of a person is drawn as.
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .rounded_full()
                    .border_2()
                    .border_color(Hsla::from(colors.bone).opacity(0.8)),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    this.picture_drag = Some(event.position);
                    cx.stop_propagation();
                }),
            )
            .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, cx| {
                let Some(from) = this.picture_drag else {
                    return;
                };
                if event.pressed_button != Some(MouseButton::Left) {
                    this.picture_drag = None;
                    return;
                }
                let moved: Point<f32> = Point {
                    x: f32::from(event.position.x - from.x),
                    y: f32::from(event.position.y - from.y),
                };
                this.picture_drag = Some(event.position);
                // The picture follows the pointer: the square moves the other way.
                let by = (
                    -moved.x * design_per_window_px / scale,
                    -moved.y * design_per_window_px / scale,
                );
                this.state
                    .update(cx, |state, cx| state.move_picture(by, cx));
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.picture_drag = None),
            )
            .on_scroll_wheel(cx.listener(move |this, event: &ScrollWheelEvent, window, cx| {
                let lines = f32::from(event.delta.pixel_delta(window.line_height()).y);
                let zoom = (crop.zoom * (1.0 + lines * 0.002)).clamp(1.0, MOST_ZOOM);
                this.state
                    .update(cx, |state, cx| state.zoom_picture(zoom, cx));
            }));
        div()
            .id("picture-crop-layer")
            .absolute()
            .inset_0()
            .child(
                Dialog::new(title, &self.picture_focus)
                    .selector("picture-crop")
                    .on_close(Self::handler(cx, |this, _, cx| {
                        this.state.update(cx, |state, cx| state.cancel_picture(cx));
                    }))
                    .child(
                        div()
                            .mb(u(10.0))
                            .typeface(theme.types.small, &theme)
                            .ink(colors.ink_2)
                            .child(text::PICTURE_CROP_HELP),
                    )
                    .child(frame)
                    .child(
                        div().mt(u(12.0)).child(
                            LevelSlider::new(
                                "picture-zoom",
                                &zoom_state,
                                &zoom_focus,
                                text::PICTURE_ZOOM,
                                ZOOM_PERCENT,
                            )
                            .selector("picture-zoom"),
                        ),
                    )
                    .child(dialog_footer([
                        Button::new("picture-crop-cancel")
                            .label(text::CANCEL)
                            .selector("picture-crop-cancel")
                            .h(u(30.0))
                            .px(u(12.0))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.state.update(cx, |state, cx| state.cancel_picture(cx));
                            }))
                            .into_any_element(),
                        Button::new("picture-crop-save")
                            .kind(ButtonKind::Primary)
                            .label(if saving {
                                text::PICTURE_SAVING
                            } else {
                                text::PICTURE_SAVE
                            })
                            .selector("picture-crop-save")
                            .h(u(30.0))
                            .px(u(12.0))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !saving {
                                    this.state.update(cx, |state, cx| state.save_picture(cx));
                                }
                            }))
                            .into_any_element(),
                    ])),
            )
            .into_any_element()
    }
}
