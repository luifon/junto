//! The state that gpui puts back when a panic passes through it (ADR 0757), with a view of
//! the test that uses gpui alone. Each panic happens during a press that the test window
//! gives to its input callback, as a platform does, so gpui stops it. Nothing draws the
//! window again between the two presses of a test: the second press uses the state that the
//! first one left.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    AppContext as _, BorrowAppContext as _, EventEmitter, Global, TestAppContext, VisualTestContext,
};

use crate::harness::{dispatch_from_the_window, draw_from_a_task, open_view, pad, press_pad};

gpui::actions!(gpui_panics, [Probe]);

const MESSAGE: &str = "a panic of the gpui test";

/// Panics the first time.
#[derive(Clone, Default)]
struct Once(Rc<Cell<bool>>);

impl Once {
    fn panic_the_first_time(&self) {
        if !self.0.replace(true) {
            panic!("{MESSAGE}");
        }
    }
}

/// Two presses with one panic between them, which gpui stops.
fn twice(cx: &mut VisualTestContext) {
    let before = junto_client::panics::panics_on_this_thread();
    press_pad(cx);
    press_pad(cx);
    assert_eq!(junto_client::panics::panics_on_this_thread(), before + 1);
}

/// A mouse listener that panics: the listeners of the drawn frame return to it.
#[gpui::test]
fn the_mouse_listeners_stay_after_a_panic_in_one(cx: &mut TestAppContext) {
    let once = Once::default();
    let runs = Rc::new(Cell::new(0));
    let (panics, counted) = (once.clone(), runs.clone());
    let mut cx = pad(
        cx,
        Rc::new(move |_, _| {
            panics.panic_the_first_time();
            counted.set(counted.get() + 1);
        }),
    );
    twice(&mut cx);
    assert_eq!(runs.get(), 1, "the second press reaches the listener");
}

struct Emitter;

struct Ping;

impl EventEmitter<Ping> for Emitter {}

/// A subscription whose callback panics: the subscribers of the emitter return to it.
#[gpui::test]
fn a_subscriber_stays_after_a_panic_in_its_callback(cx: &mut TestAppContext) {
    let emitter = cx.new(|_| Emitter);
    let once = Once::default();
    let runs = Rc::new(Cell::new(0));
    let (panics, counted) = (once.clone(), runs.clone());
    let _subscription = cx.update(|cx| {
        cx.subscribe(&emitter, move |_, _: &Ping, _| {
            panics.panic_the_first_time();
            counted.set(counted.get() + 1);
        })
    });
    let held = emitter.clone();
    let mut cx = pad(
        cx,
        Rc::new(move |_, cx| held.update(cx, |_, cx| cx.emit(Ping))),
    );
    twice(&mut cx);
    assert_eq!(runs.get(), 1, "the second emission reaches the subscriber");
}

/// A global action listener that panics: the listeners of the action return.
#[gpui::test]
fn a_global_action_listener_stays_after_a_panic_in_it(cx: &mut TestAppContext) {
    let once = Once::default();
    let runs = Rc::new(Cell::new(0));
    let (panics, counted) = (once.clone(), runs.clone());
    cx.update(|cx| {
        cx.on_action(move |_: &Probe, _| {
            panics.panic_the_first_time();
            counted.set(counted.get() + 1);
        });
    });
    // The action goes from the window, which has no listener of its own, to the global
    // listeners.
    let mut cx = pad(cx, dispatch_from_the_window(Box::new(Probe)));
    twice(&mut cx);
    assert_eq!(runs.get(), 1, "the second dispatch reaches the listener");
}

struct Store(usize);

impl Global for Store {}

/// An update of a global that panics: the global returns to the application.
#[gpui::test]
fn a_global_stays_after_a_panic_in_its_update(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_global(Store(0)));
    let once = Once::default();
    let mut cx = pad(
        cx,
        Rc::new(move |_, cx| {
            let once = once.clone();
            cx.update_global::<Store, _>(move |store, _| {
                store.0 += 1;
                once.panic_the_first_time();
            });
        }),
    );
    twice(&mut cx);
    assert_eq!(cx.update(|_, cx| cx.global::<Store>().0), 2);
}

/// A leaf whose measure in the layout panics once after `armed` is set.
struct Measured {
    armed: Rc<Cell<bool>>,
    measures: Rc<Cell<usize>>,
}

impl gpui::IntoElement for Measured {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl gpui::Element for Measured {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<gpui::ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut gpui::Window,
        _: &mut gpui::App,
    ) -> (gpui::LayoutId, ()) {
        let (armed, measures) = (self.armed.clone(), self.measures.clone());
        let id = window.request_measured_layout(gpui::Style::default(), move |_, _, _, _| {
            if armed.replace(false) {
                panic!("{MESSAGE}");
            }
            measures.set(measures.get() + 1);
            gpui::size(gpui::px(10.0), gpui::px(10.0))
        });
        (id, ())
    }

    fn prepaint(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _: gpui::Bounds<gpui::Pixels>,
        _: &mut (),
        _: &mut gpui::Window,
        _: &mut gpui::App,
    ) {
    }

    fn paint(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _: gpui::Bounds<gpui::Pixels>,
        _: &mut (),
        _: &mut (),
        _: &mut gpui::Window,
        _: &mut gpui::App,
    ) {
    }
}

struct MeasuredView {
    armed: Rc<Cell<bool>>,
    measures: Rc<Cell<usize>>,
}

impl gpui::Render for MeasuredView {
    fn render(
        &mut self,
        _: &mut gpui::Window,
        _: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        use gpui::{ParentElement as _, Styled as _};
        gpui::div().size_full().child(Measured {
            armed: self.armed.clone(),
            measures: self.measures.clone(),
        })
    }
}

/// A measure that panics inside `compute_layout`: the layout engine returns to the window, and
/// the next draw lays the view out again.
#[gpui::test]
fn the_layout_engine_stays_after_a_panic_in_a_measure(cx: &mut TestAppContext) {
    let armed = Rc::new(Cell::new(false));
    let measures = Rc::new(Cell::new(0));
    let (view_armed, view_measures) = (armed.clone(), measures.clone());
    let mut cx = open_view(cx, move |_, _| MeasuredView {
        armed: view_armed,
        measures: view_measures,
    });
    let before = junto_client::panics::panics_on_this_thread();
    armed.set(true);
    draw_from_a_task(&mut cx);
    assert_eq!(junto_client::panics::panics_on_this_thread(), before + 1);
    let measured = measures.get();
    draw_from_a_task(&mut cx);
    assert!(
        measures.get() > measured,
        "the next draw measures the view again"
    );
    assert_eq!(junto_client::panics::panics_on_this_thread(), before + 1);
}

/// An input handler of text whose replacement panics the first time.
struct Typing {
    once: Once,
    typed: Rc<std::cell::RefCell<String>>,
}

impl gpui::InputHandler for Typing {
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut gpui::Window,
        _: &mut gpui::App,
    ) -> Option<gpui::UTF16Selection> {
        None
    }

    fn marked_text_range(
        &mut self,
        _: &mut gpui::Window,
        _: &mut gpui::App,
    ) -> Option<std::ops::Range<usize>> {
        None
    }

    fn text_for_range(
        &mut self,
        _: std::ops::Range<usize>,
        _: &mut Option<std::ops::Range<usize>>,
        _: &mut gpui::Window,
        _: &mut gpui::App,
    ) -> Option<String> {
        None
    }

    fn replace_text_in_range(
        &mut self,
        _: Option<std::ops::Range<usize>>,
        text: &str,
        _: &mut gpui::Window,
        _: &mut gpui::App,
    ) {
        self.once.panic_the_first_time();
        self.typed.borrow_mut().push_str(text);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<std::ops::Range<usize>>,
        _: &str,
        _: Option<std::ops::Range<usize>>,
        _: &mut gpui::Window,
        _: &mut gpui::App,
    ) {
    }

    fn unmark_text(&mut self, _: &mut gpui::Window, _: &mut gpui::App) {}

    fn bounds_for_range(
        &mut self,
        _: std::ops::Range<usize>,
        _: &mut gpui::Window,
        _: &mut gpui::App,
    ) -> Option<gpui::Bounds<gpui::Pixels>> {
        None
    }

    fn character_index_for_point(
        &mut self,
        _: gpui::Point<gpui::Pixels>,
        _: &mut gpui::Window,
        _: &mut gpui::App,
    ) -> Option<usize> {
        None
    }
}

/// The platform gives text to the input handler of a window outside any update of gpui, as an
/// input method does on macOS and Windows. A panic in the handler becomes an error for the
/// platform, the window returns to the application, and the next text reaches the handler.
#[gpui::test]
fn the_text_input_handler_takes_text_again_after_a_panic_in_it(cx: &mut TestAppContext) {
    let typed = Rc::new(std::cell::RefCell::new(String::new()));
    let mut cx = pad(cx, Rc::new(|_, _| {}));
    let before = junto_client::panics::panics_on_this_thread();
    let window_cx = cx.update(|window, cx| window.to_async(cx));
    let mut handler = gpui::PlatformInputHandler::new(
        window_cx,
        Box::new(Typing {
            once: Once::default(),
            typed: typed.clone(),
        }),
    );
    handler.replace_text_in_range(None, "a");
    handler.replace_text_in_range(None, "b");
    assert_eq!(*typed.borrow(), "b");
    assert_eq!(junto_client::panics::panics_on_this_thread(), before + 1);
}
