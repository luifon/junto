use std::cell::{Cell, RefCell};
use std::rc::Rc;

use std::time::Instant;

use gpui::{
    App, DispatchPhase, Div, Hsla, InteractiveElement, IntoElement, MouseDownEvent, MouseMoveEvent,
    ParentElement, Pixels, RenderOnce, SharedString, Styled, Window, canvas, div,
    prelude::FluentBuilder,
};

use super::{StyleExt, icon};
use crate::assets::Icon;
use crate::theme::{theme, u};

/// The space of the rows that show a form or a question in place of their content (ADR
/// 0730). `M` names the form or question of one row.
///
/// The answer of the server closes a form at a time that the person does not choose, and a
/// row without its form has less height than the row with it. While a form is open its row
/// is measured; when the server did the request of the form, or a change of the state puts
/// the form away, the row keeps that height at least, with its content at the top. The
/// owner of the space releases it when the person opens or puts away a form or question of
/// the page, when the page changes and when the layer closes. The space releases itself
/// when the pointer moves above the row or beside it, where no control under the row is
/// under the pointer. The answer therefore moves no control under the pointer.
///
/// The kept space under the content of the row states the result of the request, in a
/// frame of the colour of a result that fills it, so that it is no empty band (finding of
/// the live tests: a large empty space stayed after an action in an expanded row). The space
/// keeps the height of the form for at most [`super::WAIT_BOUND`]; then the row has its
/// content and the result at the height of its sentence, and a press in the rows that
/// moved under a resting pointer is refused for [`super::MOVED_GUARD`] (review L2: the
/// frame was much taller than its sentence).
pub struct FormSpace<M> {
    /// The open form and the height of its row as it was drawn last.
    drawn: Rc<Cell<Option<(M, Pixels)>>>,
    /// The form whose request the server did, and the height that its row had then.
    kept: Rc<Cell<Option<(M, Pixels)>>>,
    /// The sentence of the result of the kept form.
    result: Rc<RefCell<SharedString>>,
    /// The height of the rows without their form as they were drawn last, by the first
    /// form of each row: a form is never lower than its row, so the row does not grow
    /// when the answer closes the form.
    closed: Rc<RefCell<Vec<(M, Pixels)>>>,
    /// The bound of the kept height: the number of the last kept result, the number whose
    /// timer runs, the kept result has the height of its sentence, and when that change
    /// moved the rows under a resting pointer.
    bound: Rc<Cell<Bound>>,
}

#[derive(Debug, Clone, Copy, Default)]
struct Bound {
    kept: u64,
    timed: u64,
    over: bool,
    moved_at: Option<Instant>,
}

/// The most rows whose height without a form is kept.
const MOST_CLOSED: usize = 256;

impl<M> Default for FormSpace<M> {
    fn default() -> Self {
        Self {
            drawn: Rc::default(),
            kept: Rc::default(),
            result: Rc::default(),
            closed: Rc::default(),
            bound: Rc::default(),
        }
    }
}

impl<M: Copy + PartialEq + 'static> FormSpace<M> {
    /// The row that shows the form or the question of one of `modes` while it is open.
    /// `open` is the form or question that the page shows now.
    pub fn row(&self, open: Option<M>, modes: &[M], row: impl IntoElement) -> Div {
        // The row is in a part that does not grow, so that a row that grows in its parent
        // keeps its own height and the kept height is free space under it. Both parts have
        // the width of their place: as the root of a list item, a part without a width took
        // the least width of its content, and a line of a transcript was wider than its panel.
        let space = div()
            .flex()
            .flex_col()
            .flex_none()
            .w_full()
            .child(div().flex().flex_col().flex_none().w_full().child(row));
        let key = modes.first().copied();
        if let Some(open) = open.filter(|open| modes.contains(open)) {
            let drawn = self.drawn.clone();
            let closed = key.and_then(|key| {
                self.closed
                    .borrow()
                    .iter()
                    .find(|(row, _)| *row == key)
                    .map(|(_, height)| *height)
            });
            return space
                .relative()
                .when_some(closed, |space, height| space.min_h(height))
                .child(
                    canvas(
                        move |bounds, _, _| drawn.set(Some((open, bounds.size.height))),
                        |_, (), _, _| {},
                    )
                    .absolute()
                    .size_full(),
                );
        }
        match self.kept.get() {
            Some((kept, height)) if modes.contains(&kept) => {
                let held = self.kept.clone();
                let result = self.result.borrow().clone();
                let bound = self.bound.clone();
                let over = bound.get().over;
                let guard = bound.clone();
                space
                    .when(!over, |space| space.min_h(height))
                    .relative()
                    // The result fills the space under the content and never makes the row
                    // taller than the kept height; after the bound it has the height of its
                    // sentence.
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .map(|part| {
                                if over {
                                    part.flex_none()
                                } else {
                                    part.flex_grow(1.0)
                                        .flex_shrink_0()
                                        .flex_basis(gpui::px(0.0))
                                        .min_h_0()
                                        .overflow_hidden()
                                }
                            })
                            .child(KeptResult { sentence: result }),
                    )
                    .child(
                        canvas(
                            move |area, window, cx| {
                                let mut now = bound.get();
                                if now.over || now.timed == now.kept {
                                    return;
                                }
                                now.timed = now.kept;
                                bound.set(now);
                                let (number, timer) = (now.kept, bound.clone());
                                window
                                    .spawn(cx, async move |cx| {
                                        cx.background_executor().timer(super::WAIT_BOUND).await;
                                        let _ = cx.update(|window, cx| {
                                            let mut now = timer.get();
                                            if now.kept == number && !now.over {
                                                now.over = true;
                                                // Only a pointer that rests in the rows that
                                                // move guards the next press.
                                                let at = window.mouse_position();
                                                let under = at.y >= area.top()
                                                    && at.x >= area.left()
                                                    && at.x <= area.right();
                                                if under && window.is_window_active() {
                                                    now.moved_at =
                                                        Some(cx.background_executor().now());
                                                }
                                                timer.set(now);
                                                window.refresh();
                                            }
                                        });
                                    })
                                    .detach();
                            },
                            move |bounds, (), window, _| {
                                let guarded = guard.clone();
                                window.on_mouse_event(
                                    move |event: &MouseDownEvent, phase, _, cx| {
                                        let at = event.position;
                                        let beside = at.x < bounds.left() || at.x > bounds.right();
                                        let moved = guarded.get().moved_at.is_some_and(|at| {
                                            cx.background_executor()
                                                .now()
                                                .saturating_duration_since(at)
                                                < super::MOVED_GUARD
                                        });
                                        if phase == DispatchPhase::Capture
                                            && moved
                                            && !beside
                                            && at.y >= bounds.top()
                                        {
                                            cx.stop_propagation();
                                        }
                                    },
                                );
                                window.on_mouse_event(
                                    move |event: &MouseMoveEvent, phase, window, _| {
                                        let at = event.position;
                                        let beside = at.x < bounds.left() || at.x > bounds.right();
                                        if phase == DispatchPhase::Bubble
                                            && (at.y < bounds.top() || beside)
                                            && held.get().is_some()
                                        {
                                            held.set(None);
                                            window.refresh();
                                        }
                                    },
                                );
                            },
                        )
                        .absolute()
                        .size_full(),
                    )
            }
            _ => match key {
                Some(key) => {
                    let closed = self.closed.clone();
                    space.relative().child(
                        canvas(
                            move |bounds, _, _| {
                                let mut rows = closed.borrow_mut();
                                rows.retain(|(row, _)| *row != key);
                                if rows.len() >= MOST_CLOSED {
                                    rows.remove(0);
                                }
                                rows.push((key, bounds.size.height));
                            },
                            |_, (), _, _| {},
                        )
                        .absolute()
                        .size_full(),
                    )
                }
                None => space,
            },
        }
    }

    /// The server did the request of the open form `open`, which closes now: its row keeps
    /// the height that it was drawn with, and states `result` in the space under its
    /// content.
    pub fn keep(&mut self, open: M, result: impl Into<SharedString>) {
        self.kept
            .set(self.drawn.get().filter(|(drawn, _)| *drawn == open));
        *self.result.borrow_mut() = result.into();
        let mut bound = self.bound.get();
        bound.kept += 1;
        bound.over = false;
        bound.moved_at = None;
        self.bound.set(bound);
    }

    /// The person opened or put away a form or question, the page changed or the layer
    /// closed: no row keeps a height.
    pub fn release(&mut self) {
        self.kept.set(None);
    }
}

/// The result of a request in the space that its row keeps.
#[derive(gpui::IntoElement)]
struct KeptResult {
    sentence: SharedString,
}

impl RenderOnce for KeptResult {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = theme(cx);
        let colors = theme.colors;
        div()
            .debug_selector(|| "kept-result".to_owned())
            .relative()
            .child(super::area(super::KEPT))
            .flex()
            .flex_1()
            .min_h_0()
            .items_start()
            .gap(u(8.0))
            .mt(u(6.0))
            .mb(u(6.0))
            .px(u(10.0))
            .py(u(8.0))
            .rounded(u(theme.metrics.radius))
            .border_1()
            .border_color(Hsla::from(colors.rule))
            .bg(Hsla::from(colors.cypress_wash))
            .typeface(theme.types.notice, &theme)
            .ink(colors.ink)
            .child(icon(Icon::Check, 15.0, colors.cypress_ink))
            .child(div().flex_1().min_w_0().child(self.sentence))
    }
}

/// The rows that the requests of a form add to the top of a list whose newest rows are first
/// (ADR 0730). The state of the server can bring such a row before or after the answer, and
/// before or after the person releases the space of the form, at a time that the person does
/// not choose, and a row at the top moves every row under it. A row is therefore not drawn
/// in the list from the moment its request is sent until the list is drawn after a release
/// with the row in it: the result of the form, such as a new code, is drawn in the row of
/// the form. A row that arrives after the release waits for the next release, also while a
/// further request of the form is sent.
///
/// The owner releases it where it releases its [`FormSpace`], and resets it when the page
/// changes and when the layer closes. While a request is sent, the page offers no release,
/// and the owner gives it the answer even when the form was put away: a reset drops the
/// answer of the request.
pub struct NewRow<R> {
    /// While a request is sent: the rows of the list when the person sent it, or none when
    /// the list was not read. A row that is not one of them can be its new row.
    sent: Option<Option<Vec<R>>>,
    /// The rows that answers made and that the list does not draw yet.
    made: Vec<Made<R>>,
}

struct Made<R> {
    row: R,
    /// The person released the space after the row was made and since the list was drawn
    /// last.
    released: bool,
}

impl<R> Default for NewRow<R> {
    fn default() -> Self {
        Self {
            sent: None,
            made: Vec::new(),
        }
    }
}

impl<R: Copy + PartialEq> NewRow<R> {
    /// The person sent the form while the list had the rows `listed`, or none when the
    /// list was not read.
    pub fn send(&mut self, listed: Option<Vec<R>>) {
        self.sent = Some(listed);
    }

    /// Whether a request was sent and its answer was not given to [`NewRow::answer`].
    pub fn waits(&self) -> bool {
        self.sent.is_some()
    }

    /// The server did the request and made the row `made`, or refused it.
    pub fn answer(&mut self, made: Option<R>) {
        self.sent = None;
        if let Some(row) = made
            && !self.made.iter().any(|held| held.row == row)
        {
            self.made.push(Made {
                row,
                released: false,
            });
        }
    }

    /// The person opened or put away a form, a question or the result of a form: each row
    /// made before joins the list at its next drawing when the state holds it then.
    pub fn release(&mut self) {
        for held in &mut self.made {
            held.released = true;
        }
    }

    /// The page changed or the layer closed. The list is read again when its page opens,
    /// after the answer, and draws every row that it reads.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// The rows that the list draws, of the rows `rows` of the state, in their order. `id`
    /// names a row.
    pub fn drawn<'a, T>(&mut self, rows: &'a [T], id: impl Fn(&T) -> R) -> Vec<&'a T> {
        let mut joined = Vec::new();
        self.made.retain_mut(|held| {
            if !held.released {
                return true;
            }
            held.released = false;
            if rows.iter().any(|row| id(row) == held.row) {
                joined.push(held.row);
                return false;
            }
            true
        });
        // A row that joins the list stays drawn while a further request is sent.
        if let Some(Some(listed)) = &mut self.sent {
            listed.extend(joined);
        }
        rows.iter().filter(|&row| self.shows(id(row))).collect()
    }

    fn shows(&self, row: R) -> bool {
        if self.made.iter().any(|held| held.row == row) {
            return false;
        }
        match &self.sent {
            Some(Some(listed)) => listed.contains(&row),
            Some(None) | None => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::NewRow;

    fn drawn(held: &mut NewRow<u8>, rows: &[u8]) -> Vec<u8> {
        held.drawn(rows, |row| *row).into_iter().copied().collect()
    }

    #[test]
    fn a_row_that_arrives_while_the_request_is_sent_waits_for_a_release() {
        let mut held = NewRow::default();
        held.send(Some(vec![1]));
        assert_eq!(drawn(&mut held, &[2, 1]), [1]);
        held.answer(Some(2));
        assert_eq!(drawn(&mut held, &[2, 1]), [1]);
        held.release();
        assert_eq!(drawn(&mut held, &[2, 1]), [2, 1]);
        assert_eq!(drawn(&mut held, &[3, 2, 1]), [3, 2, 1]);
    }

    #[test]
    fn a_row_of_another_person_joins_the_list_with_the_answer() {
        let mut held = NewRow::default();
        held.send(Some(vec![1]));
        assert_eq!(drawn(&mut held, &[3, 1]), [1]);
        held.answer(Some(2));
        assert_eq!(drawn(&mut held, &[3, 2, 1]), [3, 1]);
    }

    #[test]
    fn a_row_that_arrives_after_a_release_waits_for_the_next_release() {
        let mut held = NewRow::default();
        held.send(Some(vec![1]));
        held.answer(Some(2));
        assert_eq!(drawn(&mut held, &[1]), [1]);
        held.release();
        assert_eq!(drawn(&mut held, &[1]), [1]);
        assert_eq!(drawn(&mut held, &[2, 1]), [1]);
        assert_eq!(drawn(&mut held, &[2, 1]), [1]);
        held.release();
        assert_eq!(drawn(&mut held, &[2, 1]), [2, 1]);
    }

    #[test]
    fn a_release_while_the_request_is_sent_keeps_the_rows_of_the_send() {
        let mut held = NewRow::default();
        held.send(Some(vec![1]));
        held.release();
        assert_eq!(drawn(&mut held, &[2, 1]), [1]);
        held.answer(Some(2));
        held.release();
        assert_eq!(drawn(&mut held, &[2, 1]), [2, 1]);
    }

    #[test]
    fn a_refusal_and_a_reset_hold_no_row() {
        let mut held = NewRow::default();
        held.send(Some(vec![1]));
        held.answer(None);
        assert_eq!(drawn(&mut held, &[2, 1]), [2, 1]);
        held.send(Some(vec![1]));
        held.reset();
        assert_eq!(drawn(&mut held, &[2, 1]), [2, 1]);
        held.send(Some(vec![1]));
        held.answer(Some(2));
        held.reset();
        assert_eq!(drawn(&mut held, &[2, 1]), [2, 1]);
    }

    #[test]
    fn a_second_request_keeps_the_first_row_until_a_release_at_which_the_state_holds_it() {
        let mut held = NewRow::default();
        held.send(Some(vec![1]));
        held.answer(Some(2));
        held.release();
        assert_eq!(drawn(&mut held, &[1]), [1]);
        held.send(Some(vec![1]));
        assert_eq!(drawn(&mut held, &[2, 1]), [1]);
        held.answer(Some(3));
        assert_eq!(drawn(&mut held, &[3, 2, 1]), [1]);
        held.release();
        assert_eq!(drawn(&mut held, &[2, 1]), [2, 1]);
        assert_eq!(drawn(&mut held, &[3, 2, 1]), [2, 1]);
        held.release();
        assert_eq!(drawn(&mut held, &[3, 2, 1]), [3, 2, 1]);
    }

    #[test]
    fn a_release_draws_the_rows_that_the_state_holds_and_keeps_the_others() {
        let mut held = NewRow::default();
        held.send(None);
        held.answer(Some(2));
        held.send(None);
        held.answer(Some(3));
        held.release();
        assert_eq!(drawn(&mut held, &[2, 1]), [2, 1]);
        assert_eq!(drawn(&mut held, &[3, 2, 1]), [2, 1]);
        held.release();
        assert_eq!(drawn(&mut held, &[3, 2, 1]), [3, 2, 1]);
    }

    #[test]
    fn a_release_before_a_row_is_made_does_not_draw_it() {
        let mut held = NewRow::default();
        held.release();
        held.send(None);
        held.answer(Some(2));
        assert_eq!(drawn(&mut held, &[2, 1]), [1]);
        held.release();
        assert_eq!(drawn(&mut held, &[2, 1]), [2, 1]);
    }

    #[test]
    fn a_row_that_joins_the_list_while_a_further_request_is_sent_stays_drawn() {
        let mut held = NewRow::default();
        held.send(Some(vec![1]));
        held.answer(Some(2));
        held.release();
        assert_eq!(drawn(&mut held, &[1]), [1]);
        held.send(Some(vec![1]));
        assert_eq!(drawn(&mut held, &[2, 1]), [1]);
        held.release();
        assert_eq!(drawn(&mut held, &[2, 1]), [2, 1]);
        held.answer(Some(3));
        assert_eq!(drawn(&mut held, &[3, 2, 1]), [2, 1]);
    }

    #[test]
    fn a_list_that_was_not_read_at_the_send_draws_every_row() {
        let mut held = NewRow::default();
        held.send(None);
        assert_eq!(drawn(&mut held, &[2, 1]), [2, 1]);
    }
}
