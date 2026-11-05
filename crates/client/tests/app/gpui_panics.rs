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

use crate::harness::{
    dispatch_from_the_window, drag, draw_from_a_task, native_frame, open_view, pad, press_pad,
    type_text,
};

gpui::actions!(gpui_panics, [Probe]);

const MESSAGE: &str = "a panic of the gpui test";

#[gpui::test]
fn a_nested_frame_preserves_pending_picture_callbacks(cx: &mut TestAppContext) {
    let mut window = pad(cx, Rc::new(|_, _| {}));
    let ran = Rc::new(Cell::new(false));
    let (send, mut received) = futures_channel::oneshot::channel();
    window.update(|window, _| {
        let ran = ran.clone();
        window.on_next_frame(move |_, _| {
            ran.set(true);
            let _ = send.send(());
        });
    });
    native_frame(&mut window, true);
    assert!(!ran.get());
    assert!(
        received.try_recv().is_ok(),
        "the frame dropped the picture acknowledgement"
    );
    native_frame(&mut window, false);
    assert!(ran.get());
    assert_eq!(received.try_recv(), Ok(Some(())));
}

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

/// The phase of a draw in which an element of the test panics.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Phase {
    Prepaint,
    Paint,
}

/// A leaf of 10 by 10 pixels that panics once in the phase that `armed` names, and counts its
/// paints.
#[derive(Clone, Default)]
struct Phased {
    armed: Rc<Cell<Option<Phase>>>,
    paints: Rc<Cell<usize>>,
}

impl Phased {
    fn panic_once_in(&self, phase: Phase) {
        if self.armed.get() == Some(phase) {
            self.armed.set(None);
            panic!("{MESSAGE}");
        }
    }
}

impl gpui::IntoElement for Phased {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl gpui::Element for Phased {
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
        cx: &mut gpui::App,
    ) -> (gpui::LayoutId, ()) {
        let style = gpui::Style {
            size: gpui::size(gpui::px(10.0).into(), gpui::px(10.0).into()),
            ..gpui::Style::default()
        };
        (window.request_layout(style, [], cx), ())
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
        self.panic_once_in(Phase::Prepaint);
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
        self.panic_once_in(Phase::Paint);
        self.paints.set(self.paints.get() + 1);
    }
}

/// A view with a text input handler, given to the window in the paint of a canvas before the
/// element of the test, and the element of the test.
struct PhasedView {
    phased: Phased,
    focus: gpui::FocusHandle,
    typed: Rc<std::cell::RefCell<String>>,
}

impl gpui::Render for PhasedView {
    fn render(
        &mut self,
        _: &mut gpui::Window,
        _: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        use gpui::{ParentElement as _, Styled as _};
        let (focus, typed) = (self.focus.clone(), self.typed.clone());
        gpui::div()
            .size_full()
            .child(
                gpui::canvas(
                    |_, _, _| (),
                    move |_, (), window, cx| {
                        window.handle_input(
                            &focus,
                            Typing {
                                once: Once(Rc::new(Cell::new(true))),
                                typed: typed.clone(),
                            },
                            cx,
                        );
                    },
                )
                .size(gpui::px(10.0)),
            )
            .child(self.phased.clone())
    }
}

fn phased_view(
    cx: &mut TestAppContext,
) -> (VisualTestContext, Phased, Rc<std::cell::RefCell<String>>) {
    let phased = Phased::default();
    let typed = Rc::new(std::cell::RefCell::new(String::new()));
    let (view_phased, view_typed) = (phased.clone(), typed.clone());
    let mut cx = open_view(cx, move |window, cx| {
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        PhasedView {
            phased: view_phased,
            focus,
            typed: view_typed,
        }
    });
    draw_from_a_task(&mut cx);
    (cx, phased, typed)
}

/// A panic in the prepaint or in the paint of an element: gpui stops it, and the next draw
/// prepaints and paints the window again without another panic.
#[gpui::test]
fn a_draw_that_panics_in_prepaint_or_paint_is_followed_by_a_whole_draw(cx: &mut TestAppContext) {
    for phase in [Phase::Prepaint, Phase::Paint] {
        let (mut cx, phased, _) = phased_view(cx);
        let before = junto_client::panics::panics_on_this_thread();
        let painted = phased.paints.get();
        assert!(painted > 0, "the first draw painted the element");
        phased.armed.set(Some(phase));
        draw_from_a_task(&mut cx);
        assert_eq!(
            junto_client::panics::panics_on_this_thread(),
            before + 1,
            "{phase:?}: the draw did not panic once"
        );
        let after_panic = phased.paints.get();
        draw_from_a_task(&mut cx);
        assert!(
            phased.paints.get() > after_panic,
            "{phase:?}: the draw after the panic did not paint the element"
        );
        assert_eq!(junto_client::panics::panics_on_this_thread(), before + 1);
    }
}

/// The input handler that the window gave the platform before a draw that panics in a paint
/// goes back to the platform window: typing reaches it after the failed draw.
#[gpui::test]
fn typing_reaches_the_input_handler_after_a_draw_that_panicked(cx: &mut TestAppContext) {
    let (mut cx, phased, typed) = phased_view(cx);
    type_text(&mut cx, "a");
    assert_eq!(*typed.borrow(), "a", "typing before the panic");
    let before = junto_client::panics::panics_on_this_thread();
    phased.armed.set(Some(Phase::Paint));
    draw_from_a_task(&mut cx);
    assert_eq!(junto_client::panics::panics_on_this_thread(), before + 1);
    type_text(&mut cx, "b");
    assert_eq!(*typed.borrow(), "ab", "typing after the failed draw");
}

/// A view that holds only the element of the test: the dragged view and the prompt.
struct Holder {
    phased: Phased,
    focus: gpui::FocusHandle,
}

impl gpui::Render for Holder {
    fn render(
        &mut self,
        _: &mut gpui::Window,
        _: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        use gpui::ParentElement as _;
        gpui::div().child(self.phased.clone())
    }
}

impl EventEmitter<gpui::PromptResponse> for Holder {}

impl gpui::Focusable for Holder {
    fn focus_handle(&self, _: &gpui::App) -> gpui::FocusHandle {
        self.focus.clone()
    }
}

/// A square that can be dragged; the dragged view holds the element of the test.
struct DragSource {
    phased: Phased,
}

impl gpui::Render for DragSource {
    fn render(
        &mut self,
        _: &mut gpui::Window,
        _: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        use gpui::{InteractiveElement as _, StatefulInteractiveElement as _, Styled as _};
        let phased = self.phased.clone();
        gpui::div()
            .id("drag-source")
            .size(gpui::px(20.0))
            .on_drag((), move |_, _, _, cx| {
                let phased = phased.clone();
                cx.new(|cx| Holder {
                    phased,
                    focus: cx.focus_handle(),
                })
            })
    }
}

/// A draw that panics in the prepaint of the dragged view, and then in the prepaint of a
/// prompt that the window renders: gpui puts the drag and the prompt back, and the next draw
/// draws each of them again.
#[gpui::test]
fn the_dragged_view_and_the_prompt_return_after_a_panic_in_their_prepaint(cx: &mut TestAppContext) {
    use gpui::{point, px};

    let phased = Phased::default();
    let source = phased.clone();
    let mut window = open_view(cx, move |_, _| DragSource { phased: source });
    draw_from_a_task(&mut window);
    drag(
        &mut window,
        point(px(5.0), px(5.0)),
        point(px(60.0), px(60.0)),
    );
    draw_from_a_task(&mut window);
    assert!(
        window.update(|_, cx| cx.has_active_drag()),
        "no drag started"
    );
    let before = junto_client::panics::panics_on_this_thread();
    let painted = phased.paints.get();
    assert!(painted > 0, "the dragged view was not drawn");
    phased.armed.set(Some(Phase::Prepaint));
    draw_from_a_task(&mut window);
    assert_eq!(junto_client::panics::panics_on_this_thread(), before + 1);
    assert!(
        window.update(|_, cx| cx.has_active_drag()),
        "the drag ended at the panic of its view"
    );
    let after_panic = phased.paints.get();
    draw_from_a_task(&mut window);
    assert!(
        phased.paints.get() > after_panic,
        "the dragged view was not drawn after the panic"
    );

    let prompted = Phased::default();
    let held = prompted.clone();
    let mut window = open_view(cx, move |_, _| DragSource {
        phased: Phased::default(),
    });
    let _answer = window.update(move |window, cx| {
        cx.set_prompt_builder(move |_, _, _, _, handle, window, cx| {
            let phased = held.clone();
            let view = cx.new(|cx| Holder {
                phased,
                focus: cx.focus_handle(),
            });
            handle.with_view(view, window, cx)
        });
        window.prompt(gpui::PromptLevel::Info, "probe", None, &["OK"], cx)
    });
    draw_from_a_task(&mut window);
    let before = junto_client::panics::panics_on_this_thread();
    assert!(prompted.paints.get() > 0, "the prompt was not drawn");
    prompted.armed.set(Some(Phase::Prepaint));
    draw_from_a_task(&mut window);
    assert_eq!(junto_client::panics::panics_on_this_thread(), before + 1);
    let after_panic = prompted.paints.get();
    draw_from_a_task(&mut window);
    assert!(
        prompted.paints.get() > after_panic,
        "the prompt was not drawn after the panic"
    );
}
