//! The drawing of a picture of a message (ADR 0740). A still picture is an image of gpui.
//! An animation is painted one frame at a time: it plays only while some part of it is in
//! view in the active window, and with reduced motion only while the pointer is over it.
//! The window is drawn again when the shown frame of an animation ends, and not at every
//! frame of the display, and at most once in [`LEAST_INTERVAL`] for all its animations: a
//! drawing renders the whole window (review M5).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, Corners, DispatchPhase, HitboxBehavior, ImageId, IntoElement, MouseMoveEvent,
    ObjectFit, Pixels, RenderImage, SharedString, Styled, StyledImage, Window, canvas, img,
};

/// How many animations the window keeps the shown frame of.
const MOST_PLAYS: usize = 256;
/// The least time between two drawings that the animations of a window ask for. A frame that
/// ends within it is shown at the next drawing, and a shorter frame is passed over.
pub const LEAST_INTERVAL: Duration = Duration::from_millis(50);

/// The delay of a frame. A frame with a delay of 10 ms or less is shown for 100 ms, as
/// browsers show it.
fn delay(image: &RenderImage, frame: usize) -> Duration {
    let delay = Duration::from(image.delay(frame));
    if delay <= Duration::from_millis(10) {
        Duration::from_millis(100)
    } else {
        delay
    }
}

struct Play {
    frame: usize,
    /// When the shown frame ends.
    due: Instant,
    /// The drawing that drew the animation last.
    drawn: u64,
    playing: bool,
}

/// The animations of the pictures of messages in one window.
#[derive(Default)]
pub(crate) struct Playback {
    drawing: Cell<u64>,
    /// The shown frame of each animation in each place where the window draws it.
    plays: RefCell<HashMap<(SharedString, ImageId), Play>>,
    /// The time at which a timer draws the window again for the animations of the last
    /// drawing.
    wake: Rc<Cell<Option<Instant>>>,
}

impl Playback {
    /// A drawing of the window starts. The animations that it draws ask for the next
    /// drawing again; a timer that an earlier drawing started for an animation that this
    /// drawing does not play draws nothing.
    pub(crate) fn drawing_starts(&self) {
        let drawing = self.drawing.get() + 1;
        self.drawing.set(drawing);
        self.wake.set(None);
        let mut plays = self.plays.borrow_mut();
        if plays.len() > MOST_PLAYS {
            let mut drawn: Vec<u64> = plays.values().map(|play| play.drawn).collect();
            drawn.sort_unstable();
            let oldest = drawn[plays.len() - MOST_PLAYS];
            plays.retain(|_, play| play.drawn >= oldest);
        }
    }

    /// The frame that this drawing shows of an animation that is in view at `now`, and when
    /// that frame ends while the animation plays. An animation that was not drawn while it
    /// played in the drawing before continues at its shown frame from `now`.
    fn step(
        &self,
        slot: &SharedString,
        image: &RenderImage,
        playing: bool,
        now: Instant,
    ) -> (usize, Option<Instant>) {
        let count = image.frame_count().max(1);
        let drawing = self.drawing.get();
        let mut plays = self.plays.borrow_mut();
        let play = plays.entry((slot.clone(), image.id)).or_insert(Play {
            frame: 0,
            due: now,
            drawn: drawing,
            playing: false,
        });
        let continued = play.playing && play.drawn + 1 >= drawing;
        if playing && !continued {
            play.due = now + delay(image, play.frame);
        } else if playing && now >= play.due {
            let whole: Duration = (0..count).map(|frame| delay(image, frame)).sum();
            if now.saturating_duration_since(play.due) >= whole {
                play.due = now;
            }
            while now >= play.due {
                play.frame = (play.frame + 1) % count;
                play.due += delay(image, play.frame);
            }
        }
        play.drawn = drawing;
        play.playing = playing;
        (play.frame.min(count - 1), playing.then_some(play.due))
    }

    /// The frame that the window showed last of an animation.
    fn shown(&self, slot: &SharedString, image: &RenderImage) -> usize {
        self.plays
            .borrow()
            .get(&(slot.clone(), image.id))
            .map_or(0, |play| play.frame)
    }

    /// Draws the window again at `at`, but not before [`LEAST_INTERVAL`] from `now`, unless a
    /// timer draws it earlier.
    fn wake_at(&self, at: Instant, now: Instant, window: &mut Window, cx: &mut App) {
        let at = at.max(now + LEAST_INTERVAL);
        if self.wake.get().is_some_and(|waiting| waiting <= at) {
            return;
        }
        self.wake.set(Some(at));
        let wake = Rc::clone(&self.wake);
        let wait = at.saturating_duration_since(now);
        window
            .spawn(cx, async move |cx| {
                cx.background_executor().timer(wait).await;
                if wake.get() == Some(at) {
                    wake.set(None);
                    let _ = cx.update(|window, _| window.refresh());
                }
            })
            .detach();
    }

    /// True while a timer will draw the window for an animation. A test uses it.
    #[cfg(feature = "test-support")]
    pub(crate) fn waits(&self) -> bool {
        self.wake.get().is_some()
    }

    /// The frames that the last drawing showed of the animations whose place contains
    /// `slot`. A test uses it.
    #[cfg(feature = "test-support")]
    pub(crate) fn shown_frames(&self, slot: &str) -> Vec<usize> {
        let drawing = self.drawing.get();
        self.plays
            .borrow()
            .iter()
            .filter(|((place, _), play)| place.contains(slot) && play.drawn == drawing)
            .map(|(_, play)| play.frame)
            .collect()
    }
}

/// A picture of a message, fitted to its element with its proportions. `slot` names the
/// place where the window draws it, so that one animation in two places has a frame for
/// each.
pub(crate) fn picture(
    image: Arc<RenderImage>,
    slot: SharedString,
    playback: &Rc<Playback>,
) -> AnyElement {
    if image.frame_count() < 2 {
        return img(image)
            .size_full()
            .object_fit(ObjectFit::Contain)
            .into_any_element();
    }
    let playback = Rc::clone(playback);
    canvas(
        |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
        move |bounds, hitbox, window, cx| {
            let visible = bounds.intersect(&window.content_mask().bounds);
            let shown = visible.size.width > Pixels::ZERO && visible.size.height > Pixels::ZERO;
            let hovered = hitbox.is_hovered(window);
            let reduced = cx.reduce_motion();
            let playing = shown && window.is_window_active() && (!reduced || hovered);
            let now = cx.background_executor().now();
            let (frame, next) = if shown {
                playback.step(&slot, &image, playing, now)
            } else {
                (playback.shown(&slot, &image), None)
            };
            let fitted = ObjectFit::Contain.get_bounds(bounds, image.size(frame));
            let _ = window.paint_image(
                bounds,
                fitted,
                Corners::default(),
                Arc::clone(&image),
                frame,
                false,
            );
            if let Some(next) = next {
                playback.wake_at(next, now, window, cx);
            }
            if shown && reduced {
                window.on_mouse_event(move |_: &MouseMoveEvent, phase, window, _| {
                    if phase == DispatchPhase::Bubble && hitbox.is_hovered(window) != hovered {
                        window.refresh();
                    }
                });
            }
        },
    )
    .size_full()
    .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn animation(delays: &[u64]) -> RenderImage {
        RenderImage::new(
            delays
                .iter()
                .map(|delay| {
                    image::Frame::from_parts(
                        image::RgbaImage::new(1, 1),
                        0,
                        0,
                        image::Delay::from_numer_denom_ms(*delay as u32, 1),
                    )
                })
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn an_animation_shows_each_frame_for_its_delay_while_it_plays() {
        let playback = Playback::default();
        let image = animation(&[100, 50, 200]);
        let slot = SharedString::from("list");
        let start = Instant::now();
        let at = |ms| start + Duration::from_millis(ms);
        let shown = |ms, playing| {
            playback.drawing_starts();
            playback.step(&slot, &image, playing, at(ms))
        };
        assert_eq!(shown(0, true), (0, Some(at(100))));
        assert_eq!(shown(99, true), (0, Some(at(100))));
        assert_eq!(shown(100, true), (1, Some(at(150))));
        assert_eq!(shown(160, true), (2, Some(at(350))));
        // After the last frame comes the first.
        assert_eq!(shown(350, true), (0, Some(at(450))));
        // A drawing late by more than a whole round shows the next frame, without
        // counting the rounds.
        assert_eq!(shown(5_000, true), (1, Some(at(5_050))));
        // Paused: the frame stays, and no drawing is asked for.
        assert_eq!(shown(5_100, false), (1, None));
        assert_eq!(shown(9_000, false), (1, None));
        // Playing again, the frame is shown for its whole delay from then.
        assert_eq!(shown(9_000, true), (1, Some(at(9_050))));
    }

    #[test]
    fn an_animation_out_of_view_continues_where_it_stopped() {
        let playback = Playback::default();
        let image = animation(&[100, 100]);
        let slot = SharedString::from("list");
        let start = Instant::now();
        let at = |ms| start + Duration::from_millis(ms);
        playback.drawing_starts();
        assert_eq!(playback.step(&slot, &image, true, at(0)).0, 0);
        playback.drawing_starts();
        assert_eq!(playback.step(&slot, &image, true, at(120)).0, 1);
        // Drawings without the animation: it was scrolled out of view.
        for _ in 0..5 {
            playback.drawing_starts();
        }
        playback.drawing_starts();
        assert_eq!(
            playback.step(&slot, &image, true, at(10_000)),
            (1, Some(at(10_100)))
        );
        // Another place of the same animation has a frame of its own.
        let other = SharedString::from("viewer");
        assert_eq!(playback.step(&other, &image, true, at(10_000)).0, 0);
    }

    #[test]
    fn a_frame_without_a_delay_is_shown_for_100_ms() {
        let image = animation(&[0, 10, 11]);
        assert_eq!(delay(&image, 0), Duration::from_millis(100));
        assert_eq!(delay(&image, 1), Duration::from_millis(100));
        assert_eq!(delay(&image, 2), Duration::from_millis(11));
    }
}
