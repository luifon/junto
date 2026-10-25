# 0757. Panic reports, the log file, and panics in the callbacks of the platform

## Status

Accepted

## Context

In the first live test the macOS client ended three times. Each time a Rust panic in a listener of the window (the scroll of the message list) ran inside `handle_view_event` of `gpui-pre-macos`, an Objective-C method that AppKit calls. A Rust function with the `extern "C"` ABI cannot unwind, so the panic became `panic in a function that cannot unwind` and the process aborted. Twice the message of the panic was lost: the client wrote its log only to standard error, which an application that the system starts does not keep, and it recorded no panic.

The places where the platform calls gpui, and so the code of the application, in `gpui-pre` 0.3.8:

- macOS: the Objective-C methods of the window, the view and the delegate of the application (`gpui-pre-macos`), and the functions that the dispatch queues of the system call to run a task, on the main thread and on the background threads. All are `extern "C"`: a panic aborts the process.
- Windows: the window procedures of the windows and of the platform (`extern "system"`); the platform runs the tasks of the main thread from its window procedure and the background tasks from the callbacks of the thread pool (`extern "system"`). A panic aborts the process.
- Linux, X11 and Wayland: the event loop of the platform is Rust code. A panic unwinds to `main` and ends the process with the exit code of a panic. For the person the result is the same: the application ends.

No profile of the workspace sets `panic = "abort"`, and no build or packaging script passes it.

gpui takes values out of the state of the application while it calls the code of the application, and puts them back after the call: the count of the nested updates of the application and the flag of the flush of the effects, the window that an update of a window uses, the entity that an update of an entity uses, the global that an update of a global uses, the subscribers of an entity while their callbacks run, the global action listeners, the mouse listeners of the drawn frame, the layout engine during a layout, and the phase of a draw. The published version does not put them back when a panic unwinds through it. A panic stopped only where the platform calls gpui would leave an application that no longer runs its effects, has lost the window and the entity of the handler that panicked, and skips every later draw.

## Decision

### The report of a panic and the log file

- `junto_client::panics::install` sets the panic hook at the start of `main`, before the arguments are read. The hook runs on the thread that panics, before the unwind, and then calls the hook that was set before it, which writes the panic to standard error as before.
- The hook writes one report file per panic into `panics/` of the data directory: `panic-<UTC time>-<process>-<number>.txt`, with the build label of the version, the time, the system and the architecture, the name and identifier of the thread, the location, the message and the backtrace (`Backtrace::force_capture`, also in a release build). The file is written with `create_new` and synchronized.
- The hook writes on the thread that panics, also on the thread of the window, which otherwise does not wait for the file system (ADR 0730): the report must exist before a panic that is not stopped ends the process, and a panic is not a part of the work of the window.
- The data directory of a report: the thread that opened the state of the application (`AppState::new`, the thread of the windows) uses the data directory of that state; every other thread uses the data directory that `main` names after the client has opened it. Before the data directory is known a panic is written to standard error only.
- Bounds: one process writes at most 10 report files; a later panic is still counted and shown, without a file. A new report removes the oldest beyond 20 in the directory. A panic that repeats at each frame then neither fills the disk nor holds the processor with backtraces.
- The log: the `tracing` output, without colour codes, goes to `logs/junto.log` of the data directory, at the level of `JUNTO_LOG_LEVEL`, or `info` when it is not set; standard error keeps its default level `warn`. When the file would pass 4 MiB it becomes `junto.log.1`, the earlier `junto.log.1` becomes `junto.log.2`, and the earlier `junto.log.2` is removed, so the log holds at most 12 MiB. `main` opens the file at the start, after the client holds the lock of the data directory, so a second instance does not write into it. The thread that logs gives each event whole to a thread of the log, `junto-log`, which writes and rotates the file, so the thread of the window does not wait for the file system (ADR 0730). At most 4096 events wait for that thread; a later event is dropped and counted, and the next written line states the count. Events that wait when the process ends are lost; the report of a panic does not depend on them.
- Nothing of either is sent over the network.
- The notice: the hook sends each report to the states of the application that watch its data directory, over a channel. The state logs the report at level `error` and holds a notice with the count of the panics since the notice was last closed and the latest report. The shell shows it over the bottom of the main window, "An error happened in Junto, and the action that was running may not have finished. The report is in <path>.", with a button that closes it.

### A panic in a callback of the platform does not end the application

The workspace uses corrected copies of `gpui-pre` 0.3.8 and `gpui-pre-scheduler` 0.3.8 under `vendor/` (`[patch.crates-io]` of `Cargo.toml`; `vendor/README.md` lists each change). The platform crates are unchanged.

1. **Stops at the entries.** Every callback that gpui gives the platform runs its body through `unwind::catch`, which stops a panic and gives the platform a fixed result: the callbacks of a window (frame request, input, resize, move, activation, visibility, hover, appearance, button layout, visual viewport, insets, close, the hit test of the window controls, the tab callbacks, should-close), of the application (open addresses, reopen, keyboard layout, thermal state, sleep, wake, quit, the response to a system notification) and of the menus. The results after a panic: an input event is not handled; the hit test finds no window control; should-close and quit return true, as when the application is gone; a menu command is not available. The methods of the text input handler, which the platform calls for typing and the input methods of the system, return an error, as for a window that is closed.
2. **Tasks.** The scheduler builds every task with `propagate_panic`. A panic of a task stops in `Runnable::run`, inside the function of the platform that runs the task; the code that awaits the task receives the panic, and the panic of a detached task is dropped. This covers the dispatch queue functions of macOS, the window procedure and the thread pool callbacks of Windows, and the event loop of Linux.
3. **The state of gpui is put back.** Each frame of gpui that takes a value out of the state while it calls other code puts the value back when a panic passes and lets the panic continue: `App::update` (the count of nested updates), `App::finish_update` (the flag of the flush; no effect runs while a panic unwinds), the update of a window (the window returns to the application), the update of an entity (the entity returns), the update of a global, `SubscriberSet::retain`, the global action listeners, the mouse listeners of the frame, and `Window::compute_layout` (the layout engine). A panic in a draw resets the draw: see below.

The stops are in gpui and not in the platform crates, because the platform reaches the code of the application only through the closures that gpui gives it, the input handler and the tasks; one copy covers macOS, Windows, X11 and Wayland, and the restore of the state of gpui is needed wherever the panic stops. A stop in each handler of Junto would miss the code of gpui elements, where the panic of the live test ran, and would need one at each of several hundred handlers. Ending the application and starting it again would end the call of the person and the state of the window.

### What a stopped panic can leave inconsistent, and what the application does

- The handler of the application stopped at the panic. Its entity holds the changes made before the panic and none after it; the application continues with that state. The notice tells the person that the action may not have finished.
- Borrows of a `RefCell` held by the frames that unwound are released by the unwind. In the live test the list held its state mutably during its scroll; that borrow ends with the unwind, and the next scroll borrows it again.
- The effects queued before the panic stay queued and run at the end of the next update; the effect that panicked does not run again.
- A draw that panicked: the frame drawn before stays on the screen with its hitboxes. The element states that the failed draw had moved to the next frame return to the drawn frame; the state of the element that panicked is lost and starts again at its default. A listener or input handler that the failed draw had moved out of the drawn frame is lost; a mouse listener that is missing is skipped until the next draw. The next draw renders every view again instead of reusing a view of the failed draw. It starts at the next change of the window, not at once, so that a view that panics at every render does not draw at every frame of the display; such a window shows its last frame.
- The callbacks for the next frame of a frame that panicked are lost.
- Not stopped: a panic in the start of the application (the closure of `Application::run`), where no window exists yet; a panic in a `Drop` that runs during an unwind, which Rust aborts; a panic on a thread that gpui does not run (the threads of the client core and of the media engine), which ends its task or thread as before. Each of them is written to a report before.

### Tests

- The panics of the window tests no longer reach the test. The harness of `crates/client/tests/app` counts the panics of the thread of the windows (`panics::panics_on_this_thread`, from the hook) and fails the test when its last window is dropped after more panics than the test expected (`Window::expect_panics`).
- `panics::a_panic_in_the_scroll_of_the_message_list_is_reported_and_the_window_continues` (`tests/app/panics.rs`, the test platform of gpui): the scroll listener of the message list panics, through the test hook `panics::test_support`, while the list holds its state. The test requires that the panic does not reach the test, the report file with the message, the location, the version and a backtrace through `feed_scrolled`, the notice in the window, then that the list scrolls and follows its end again and that the close button of the notice works. Without the copies of gpui the panic reaches the test.
- `native_panics` (`tests/native_panics/main.rs`, without the test runner, `JUNTO_TEST_NATIVE_PANICS=1`): the application on the run loop of the system with the main window of an empty data directory. A task on the thread of the windows panics; on macOS a press of the left button is then posted through `NSApplication postEvent:atStart:` and the handler of the end of the press panics inside the method of the view that AppKit calls, and a second press reaches the handler without a panic. After each panic it requires the report and the notice, and at the end a frame of the window. The macOS CI job runs it (ADR 0905); before this decision the process aborted at the first panic.
- The unit tests of `logs` check the rotation and the bound of three files.

## Consequences

- The workspace keeps two more corrected copies. A new version of gpui replaces them only after the tests above pass with it; the copies are removed when a pinned upstream version stops a panic in each callback of the platform and puts its state back.
- A panic in a handler no longer ends the application, so a defect can stay unseen; the notice and the report file make it visible to the person, who can send the report to whoever runs the server.
- The paths of the log and of the reports are in `docs/client-macos.md`, `docs/client-linux.md` and `docs/client-windows.md`.
