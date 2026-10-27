//! The places of the controls of a drawing. `junto-screens` fails on a control that lies past
//! the left or right edge of the window while no clip narrower than the window holds it, as a
//! scrolling strip does: nothing can bring such a control into view (ADR 0730). The
//! application records nothing.

use gpui::{Bounds, Pixels, SharedString};

/// Records the place of the control that holds it, with the clip that applies to it.
#[cfg(feature = "test-support")]
pub(crate) fn placed(name: SharedString) -> impl gpui::IntoElement {
    use gpui::Styled;
    gpui::canvas(
        move |bounds, window, _| {
            record::push(Placed {
                name,
                bounds,
                clip: window.content_mask().bounds,
            });
        },
        |_, (), _, _| {},
    )
    .absolute()
    .inset_0()
}

/// A control as a drawing placed it.
#[derive(Debug, Clone, PartialEq)]
pub struct Placed {
    pub name: SharedString,
    pub bounds: Bounds<Pixels>,
    /// The part of the window in which the control can be seen.
    pub clip: Bounds<Pixels>,
}

/// The controls that lie past the left or right edge of a window `width` wide, where only the
/// window clips them.
pub fn outside(controls: &[Placed], width: Pixels) -> Vec<&Placed> {
    let edge = gpui::px(0.5);
    controls
        .iter()
        .filter(|control| {
            let past_right =
                control.bounds.right() > width + edge && control.clip.right() >= width - edge;
            let past_left = control.bounds.left() < -edge && control.clip.left() <= edge;
            past_right || past_left
        })
        .collect()
}

#[cfg(feature = "test-support")]
mod record {
    use std::cell::RefCell;

    use super::Placed;

    thread_local! {
        static PLACED: RefCell<Option<Vec<Placed>>> = const { RefCell::new(None) };
    }

    pub(super) fn push(control: Placed) {
        PLACED.with(|placed| {
            if let Some(placed) = placed.borrow_mut().as_mut() {
                placed.push(control);
            }
        });
    }

    /// From now on the drawings of this thread record their controls.
    pub fn record_controls() {
        PLACED.with(|placed| *placed.borrow_mut() = Some(Vec::new()));
    }

    /// The controls that the drawings recorded since [`record_controls`]; the recording ends.
    pub fn take_controls() -> Vec<Placed> {
        PLACED.with(|placed| placed.borrow_mut().take().unwrap_or_default())
    }
}

#[cfg(feature = "test-support")]
pub use record::{record_controls, take_controls};

#[cfg(test)]
mod tests {
    use gpui::{point, px, size};

    use super::*;

    fn control(x: f32, width: f32, clip: (f32, f32)) -> Placed {
        Placed {
            name: "control".into(),
            bounds: Bounds::new(point(px(x), px(10.0)), size(px(width), px(30.0))),
            clip: Bounds::new(point(px(clip.0), px(0.0)), size(px(clip.1), px(600.0))),
        }
    }

    #[test]
    fn a_control_past_the_edge_of_the_window_is_found_and_one_in_a_strip_is_not() {
        let inside = control(680.0, 34.0, (0.0, 720.0));
        let past_right = control(875.0, 34.0, (0.0, 720.0));
        let past_left = control(-20.0, 34.0, (0.0, 720.0));
        // A scrolling strip clips its tiles to its own width.
        let in_strip = control(700.0, 96.0, (200.0, 400.0));
        let controls = [inside, past_right.clone(), past_left.clone(), in_strip];
        assert_eq!(outside(&controls, px(720.0)), vec![&past_right, &past_left]);
    }
}
