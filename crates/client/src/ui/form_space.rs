use std::cell::Cell;
use std::rc::Rc;

use gpui::{Div, IntoElement, ParentElement, Pixels, Styled, canvas, div};

/// The space of the rows that show a form or a question in place of their content (ADR
/// 0730). `M` names the form or question of one row.
///
/// The answer of the server closes a form at a time that the person does not choose, and a
/// row without its form has less height than the row with it. While a form is open its row
/// is measured; when the server did the request of the form, or a change of the state puts
/// the form away, the row keeps that height at least, with its content at the top. The
/// owner of the space releases it only when the person opens or puts away a form or
/// question of the page, when the page changes and when the layer closes. The answer
/// therefore moves no control under the row.
pub struct FormSpace<M> {
    /// The open form and the height of its row as it was drawn last.
    drawn: Rc<Cell<Option<(M, Pixels)>>>,
    /// The form whose request the server did, and the height that its row had then.
    kept: Option<(M, Pixels)>,
}

impl<M> Default for FormSpace<M> {
    fn default() -> Self {
        Self {
            drawn: Rc::default(),
            kept: None,
        }
    }
}

impl<M: Copy + PartialEq + 'static> FormSpace<M> {
    /// The row that shows the form or the question of one of `modes` while it is open.
    /// `open` is the form or question that the page shows now.
    pub fn row(&self, open: Option<M>, modes: &[M], row: impl IntoElement) -> Div {
        // The row is in a part that does not grow, so that a row that grows in its parent
        // keeps its own height and the kept height is free space under it.
        let space = div()
            .flex()
            .flex_col()
            .flex_none()
            .child(div().flex().flex_col().flex_none().child(row));
        if let Some(open) = open.filter(|open| modes.contains(open)) {
            let drawn = self.drawn.clone();
            return space.relative().child(
                canvas(
                    move |bounds, _, _| drawn.set(Some((open, bounds.size.height))),
                    |_, (), _, _| {},
                )
                .absolute()
                .size_full(),
            );
        }
        match self.kept {
            Some((kept, height)) if modes.contains(&kept) => space.min_h(height),
            _ => space,
        }
    }

    /// The server did the request of the open form `open`, which closes now: its row keeps
    /// the height that it was drawn with.
    pub fn keep(&mut self, open: M) {
        self.kept = self.drawn.get().filter(|(drawn, _)| *drawn == open);
    }

    /// The person opened or put away a form or question, the page changed or the layer
    /// closed: no row keeps a height.
    pub fn release(&mut self) {
        self.kept = None;
    }
}

/// The row that the request of a form adds to the top of a list whose newest rows are first
/// (ADR 0730). The state of the server can bring the row before or after the answer, at a
/// time that the person does not choose, and a row at the top moves every row under it. The
/// row is therefore not drawn in the list while the request is sent and while the row of
/// its form keeps its height: the result of the form, such as a new code, is drawn in the
/// row of the form. The owner releases it where it releases its [`FormSpace`].
pub struct NewRow<R> {
    /// While the request is sent: the rows of the list when the person sent it. A row that
    /// is not one of them can be the new row.
    listed: Option<Vec<R>>,
    /// The row that the answer made.
    made: Option<R>,
}

impl<R> Default for NewRow<R> {
    fn default() -> Self {
        Self {
            listed: None,
            made: None,
        }
    }
}

impl<R: Copy + PartialEq> NewRow<R> {
    /// The person sent the form while the list had the rows `listed`, or none when the
    /// list was not read.
    pub fn send(&mut self, listed: Option<Vec<R>>) {
        self.listed = listed;
        self.made = None;
    }

    /// The server did the request and made the row `made`, or refused it.
    pub fn answer(&mut self, made: Option<R>) {
        self.listed = None;
        self.made = made;
    }

    /// The person opened or put away a form, a question or the result of a form, the page
    /// changed or the layer closed: every row is drawn in the list.
    pub fn release(&mut self) {
        self.listed = None;
        self.made = None;
    }

    /// Whether the list draws the row `row`.
    pub fn shows(&self, row: R) -> bool {
        match &self.listed {
            Some(listed) => listed.contains(&row),
            None => self.made != Some(row),
        }
    }
}
