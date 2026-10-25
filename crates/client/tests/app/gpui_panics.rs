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

use crate::harness::{dispatch_from_the_window, pad, press_pad};

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
