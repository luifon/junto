//! The places of the controls of a drawing. `junto-screens` fails on a control that lies past
//! the left or right edge of the window where only the window clips it: nothing can bring such
//! a control into view (ADR 0730, ADR 0731). A scrolling strip ([`strip`]) brings each of its
//! controls into view with a scroll, so for a control in a strip the place of the strip is
//! checked. The application records nothing.

use gpui::{
    AnyElement, App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement,
    LayoutId, Pixels, SharedString, Window,
};

/// Records the place of the control that holds it, with the clip that applies to it.
#[cfg(feature = "test-support")]
pub(crate) fn placed(name: SharedString) -> impl gpui::IntoElement {
    use gpui::Styled;
    gpui::canvas(
        move |bounds, window, _| {
            let clip = window.content_mask().bounds;
            let (bounds, clip) = record::strip().unwrap_or((bounds, clip));
            record::push(Placed { name, bounds, clip });
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
    /// The bounds of the control, or of the outermost scrolling strip that holds it.
    pub bounds: Bounds<Pixels>,
    /// The part of the window in which those bounds can be seen.
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

/// A strip that scrolls its content, such as the bar of the shares of a call: a scroll of the
/// strip brings each control in it into view, also a control past the edge of the window when
/// the strip ends at that edge. The check of `junto-screens` takes the place of the strip for
/// its controls.
pub(crate) fn strip(content: impl IntoElement) -> Strip {
    Strip {
        content: content.into_any_element(),
    }
}

pub(crate) struct Strip {
    content: AnyElement,
}

impl IntoElement for Strip {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for Strip {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.content.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        #[cfg(feature = "test-support")]
        let _strip = record::Inside::strip(bounds, window.content_mask().bounds);
        #[cfg(not(feature = "test-support"))]
        let _ = bounds;
        self.content.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.content.paint(window, cx);
    }
}

#[cfg(feature = "test-support")]
mod record {
    use std::cell::{Cell, RefCell};

    use super::Placed;

    type Place = (gpui::Bounds<gpui::Pixels>, gpui::Bounds<gpui::Pixels>);

    thread_local! {
        static PLACED: RefCell<Option<Vec<Placed>>> = const { RefCell::new(None) };
        /// The bounds and the clip of the outermost strip whose content is being placed.
        static STRIP: Cell<Option<Place>> = const { Cell::new(None) };
    }

    pub(super) fn strip() -> Option<Place> {
        STRIP.get()
    }

    /// The content of a strip is being placed until this ends.
    pub(super) struct Inside(Option<Place>);

    impl Inside {
        pub(super) fn strip(
            bounds: gpui::Bounds<gpui::Pixels>,
            clip: gpui::Bounds<gpui::Pixels>,
        ) -> Self {
            let outer = STRIP.get();
            STRIP.set(Some(outer.unwrap_or((bounds, clip))));
            Self(outer)
        }
    }

    impl Drop for Inside {
        fn drop(&mut self) {
            STRIP.set(self.0);
        }
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

    struct Edges;

    impl gpui::Render for Edges {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            _: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            use gpui::{
                InteractiveElement, ParentElement, StatefulInteractiveElement, Styled, div,
            };
            // Each row ends at the right edge of the window and holds a button 300 pixels
            // after its start, past that edge.
            let row = |id: &'static str, top: f32, right: f32| {
                div()
                    .id(id)
                    .absolute()
                    .top(px(top))
                    .right(px(right))
                    .w(px(200.0))
                    .h(px(40.0))
                    .flex()
                    .child(div().flex_none().w(px(300.0)))
            };
            let button = |selector: &'static str| {
                crate::ui::Button::new(selector)
                    .label(selector)
                    .selector(selector)
            };
            div()
                .size_full()
                .relative()
                .child(strip(
                    row("strip", 0.0, 0.0)
                        .overflow_x_scroll()
                        .child(button("in-strip")),
                ))
                .child(
                    row("panel", 60.0, 0.0)
                        .overflow_hidden()
                        .child(button("in-panel")),
                )
                .child(strip(
                    row("strip-past-the-edge", 120.0, -100.0)
                        .overflow_x_scroll()
                        .child(button("in-strip-past-the-edge")),
                ))
        }
    }

    #[gpui::test]
    fn a_strip_that_ends_at_the_edge_scrolls_its_controls_into_view_and_a_panel_does_not(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_base::init(cx);
            crate::theme::install(crate::theme::Theme::stone(), cx);
        });
        let (_edges, cx) = cx.add_window_view(|_, _| Edges);
        cx.run_until_parked();
        record_controls();
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        let controls = take_controls();
        let width = cx.update(|window, _| window.viewport_size().width);
        let names: Vec<_> = outside(&controls, width)
            .into_iter()
            .map(|control| control.name.to_string())
            .collect();
        assert_eq!(names, ["in-panel", "in-strip-past-the-edge"]);
        let in_strip = controls
            .iter()
            .find(|control| control.name == "in-strip")
            .unwrap();
        assert_eq!(in_strip.bounds.right(), width);
        assert_eq!(in_strip.bounds.size.width, px(200.0));
    }
}
