//! The check of the thread of the window (ADR 0730): after the start, the thread of the
//! window takes no lock of its data directory. Every window test runs with the check; these
//! tests show where it refuses a lock and where it accepts one, and that the tests give
//! every input of a window and every run of its tasks through the harness, where the check
//! applies.

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
#[should_panic(expected = "was taken on a thread that must not wait for it")]
fn a_lock_in_an_update_of_the_application_that_the_test_makes_fails_the_test(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let handle = window.handle();
    window
        .cx
        .cx
        .update(|_| handle.set_connection_mode(ConnectionMode::WhenSelected))
        .unwrap();
}

#[gpui::test]
#[should_panic(expected = "was taken on a thread that must not wait for it")]
fn a_lock_in_a_task_of_the_application_that_the_test_runs_fails_the_test(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let handle = window.handle();
    // The task takes the lock outside an update of the application.
    window.cx.cx.update(|cx| {
        cx.spawn(async move |_| {
            handle
                .set_connection_mode(ConnectionMode::WhenSelected)
                .unwrap();
        })
        .detach();
    });
    window.run_tasks();
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
        window.run_tasks();
    });
    assert_eq!(handle.entry().connection_mode, ConnectionMode::WhenSelected);

    // The code of the test itself is no work of the window.
    handle.set_connection_mode(ConnectionMode::Always).unwrap();
    assert_eq!(handle.entry().connection_mode, ConnectionMode::Always);
}

/// gpui runs the tasks of the application at the end of a simulated input and of a dispatched
/// action, and in `run_until_parked`. Outside an update of the application only the harness
/// marks that work as work of the window, so no test does it on a test context directly.
#[test]
fn the_tests_give_input_and_run_tasks_only_through_the_harness() {
    fn files(directory: &std::path::Path, found: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                files(&path, found);
            } else if path.extension().is_some_and(|ending| ending == "rs") {
                found.push(path);
            }
        }
    }
    // The directory of this package in the checkout that runs the test, as in the check of
    // `chrono::Local` in `notices.rs`.
    let package = std::env::var_os("CARGO_MANIFEST_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            panic!("CARGO_MANIFEST_DIR is not set: run the test through cargo test")
        });
    let tests = package.join("tests");
    let harness = tests.join("app").join("harness.rs");
    let mut found = Vec::new();
    files(&tests, &mut found);
    assert!(found.len() > 30, "{found:?}");
    // A call is accepted only as the body of `Window::simulate`, whose argument is the test
    // context of the window, written with or without a block.
    const ALLOWED: [&str; 2] = [".simulate(|cx|cx.", ".simulate(|cx|{cx."];
    // The names are joined here, so that this file does not match them.
    let calls = [
        concat!("simulate", "_"),
        concat!("dispatch_action", "("),
        concat!("run_until_parked", "("),
        concat!("deactivate_window", "("),
        concat!("from_window", "("),
    ];
    let mut direct = Vec::new();
    for path in found.iter().filter(|path| **path != harness) {
        let text: String = std::fs::read_to_string(path)
            .unwrap()
            .chars()
            .filter(|letter| !letter.is_whitespace())
            .collect();
        for call in calls {
            for (at, _) in text.match_indices(call) {
                if call.starts_with("from_window")
                    || !ALLOWED.iter().any(|allowed| text[..at].ends_with(allowed))
                {
                    let start = text[..at].len().saturating_sub(40);
                    let start = (start..at).find(|index| text.is_char_boundary(*index));
                    let before = start.map_or("", |start| &text[start..at]);
                    direct.push(format!("{}: {before}{call}", path.display()));
                }
            }
        }
    }
    assert!(
        direct.is_empty(),
        "these calls run work of a window outside the harness: {direct:#?}"
    );
}
