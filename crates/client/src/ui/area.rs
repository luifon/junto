//! The areas of the screens whose parts start at their top and follow each other (ADR
//! 0730). `junto-screens` measures the empty space between the parts of each area that a
//! drawing names; the application records nothing.

use gpui::{IntoElement, SharedString, Styled, canvas};

/// Names the element that holds it as an area. The element is `relative`.
pub fn area(name: impl Into<SharedString>) -> impl IntoElement {
    let name = name.into();
    canvas(
        move |bounds, _, _| {
            #[cfg(feature = "test-support")]
            record::push(name, bounds);
            #[cfg(not(feature = "test-support"))]
            let _ = (name, bounds);
        },
        |_, (), _, _| {},
    )
    // The insets place it on the box of its element: without them it would start at the
    // static place of a child, after the padding of a flex container.
    .absolute()
    .inset_0()
}

/// The name that [`cover`] records.
pub const COVER: &str = ":cover";

/// The name of the frame of a kept result (`ui::FormSpace`): the space after its sentence
/// is measured as well.
pub const KEPT: &str = ":kept";

/// Names the element that holds it as a layer that covers what is drawn before it: the
/// areas under it are not measured.
pub fn cover() -> impl IntoElement {
    area(COVER)
}

#[cfg(feature = "test-support")]
mod record {
    use std::cell::RefCell;

    use gpui::{Bounds, Pixels, SharedString};

    /// The name and the bounds of each area that a drawing recorded.
    pub type Areas = Vec<(SharedString, Bounds<Pixels>)>;

    thread_local! {
        static AREAS: RefCell<Option<Areas>> = const { RefCell::new(None) };
    }

    pub(super) fn push(name: SharedString, bounds: Bounds<Pixels>) {
        AREAS.with(|areas| {
            if let Some(areas) = areas.borrow_mut().as_mut() {
                areas.push((name, bounds));
            }
        });
    }

    /// From now on the drawings of this thread record their areas.
    pub fn record_areas() {
        AREAS.with(|areas| *areas.borrow_mut() = Some(Vec::new()));
    }

    /// The areas that the drawings recorded since [`record_areas`]; the recording ends.
    pub fn take_areas() -> Areas {
        AREAS.with(|areas| areas.borrow_mut().take().unwrap_or_default())
    }
}

#[cfg(feature = "test-support")]
pub use record::{record_areas, take_areas};
