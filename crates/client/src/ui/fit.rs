use std::cell::Cell;
use std::rc::Rc;

use gpui::{IntoElement, ParentElement, Pixels, Rems, Styled, canvas, div, px};

#[derive(Debug, Clone, Copy, Default)]
struct Measure {
    compact: bool,
    /// The width of the row and the right end of its content area at the last drawing.
    row: Option<(Pixels, Pixels)>,
    /// The width that the row needed for the full form when it last did not fit.
    full_needs: Option<Pixels>,
}

/// A row of controls with a full and a short form, such as the header of the main area: the
/// full form with the labels of its buttons while it fits, the short form with their icons
/// otherwise, which wraps onto further lines where even it does not fit, so that no control
/// lies past the edge of the window (ADR 0730). The row measures itself at each drawing; the
/// full form comes back when the row is as wide as the full form needed.
#[derive(Debug, Clone, Default)]
pub struct Fit(Rc<Cell<Measure>>);

impl Fit {
    /// The row draws its short form.
    pub fn compact(&self) -> bool {
        self.0.get().compact
    }

    /// The first child of the row, over its whole box: it measures the row, and brings the
    /// full form back when the row is as wide as the full form needed. `padding` is the
    /// padding at the right of the row and `gap` the space between its children, which also
    /// precedes [`Fit::end`].
    pub fn row(&self, padding: Rems, gap: Rems) -> impl IntoElement {
        let measure = self.0.clone();
        canvas(
            move |bounds, window, cx| {
                let rem = window.rem_size();
                let mut now = measure.get();
                let width = bounds.size.width;
                now.row = Some((
                    width,
                    bounds.right() - padding.to_pixels(rem) + gap.to_pixels(rem),
                ));
                let full = now.compact && now.full_needs.is_none_or(|needs| width >= needs);
                if full {
                    now.compact = false;
                }
                measure.set(now);
                if full {
                    // A refresh during the drawing is lost; the next frame draws the form.
                    window.defer(cx, |window, _| window.refresh());
                }
            },
            |_, (), _, _| {},
        )
        .absolute()
        .inset_0()
    }

    /// The last child of the row in its full form: where the content ends. The short form
    /// takes over when the content ends past the row.
    pub fn end(&self) -> impl IntoElement {
        let measure = self.0.clone();
        div().flex_none().w(px(0.0)).h(px(1.0)).child(
            canvas(
                move |bounds, window, cx| {
                    let mut now = measure.get();
                    let Some((width, right)) = now.row else {
                        return;
                    };
                    let over = bounds.left() - right;
                    if now.compact || over <= px(0.5) {
                        return;
                    }
                    now.compact = true;
                    now.full_needs = Some(width + over);
                    measure.set(now);
                    // A refresh during the drawing is lost; the next frame draws the form.
                    window.defer(cx, |window, _| window.refresh());
                },
                |_, (), _, _| {},
            )
            .size_full(),
        )
    }
}
