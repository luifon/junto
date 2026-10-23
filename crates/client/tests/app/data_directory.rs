//! The check of the thread of the window (ADR 0730): after the start, the thread of the
//! window takes no lock of its data directory. Every window test runs with the check; these
//! tests show where it refuses a lock and where it accepts one.

use gpui::TestAppContext;
use junto_client_core::book::ConnectionMode;

use crate::harness::{World, member};

#[gpui::test]
#[should_panic(expected = "was taken on a thread that must not wait for it")]
fn a_lock_of_the_data_directory_in_the_work_of_the_window_fails_the_test(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let handle = window.handle();
    window
        .as_window(|_| handle.set_connection_mode(ConnectionMode::WhenSelected))
        .unwrap();
}

#[gpui::test]
fn a_lock_in_a_background_task_of_the_window_and_in_the_test_is_accepted(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let handle = window.handle();

    // A background task that the window starts runs on the thread of the test as well.
    let changed = handle.clone();
    window.as_window(|window| {
        window
            .cx
            .executor()
            .spawn(async move {
                changed
                    .set_connection_mode(ConnectionMode::WhenSelected)
                    .unwrap();
            })
            .detach();
        window.cx.run_until_parked();
    });
    assert_eq!(handle.entry().connection_mode, ConnectionMode::WhenSelected);

    // The code of the test itself is no work of the window.
    handle.set_connection_mode(ConnectionMode::Always).unwrap();
    assert_eq!(handle.entry().connection_mode, ConnectionMode::Always);
}
