# 0902. Testing strategy

## Status

Accepted

## Context

The client runs on three operating systems, and calls, recording and transcripts involve several components (ADR 0001). A call cannot be tested automatically with real people, real microphones and real screens. Real capture of screen, sound and microphone uses operating-system functions that a test participant without a user interface does not use.

## Decision

Automated tests:

- CI builds and tests on macOS, Windows and Linux for every commit.
- A test participant without a user interface publishes a known tone and a known test picture into a call. The test records the call and checks the file.
- Tests cover reconnects, a recorder that stops during a recording, and a restore from a backup (ADR 0005).
- Tests run the client against the current and the previous API version (ADR 0201).
- CI regenerates the API documentation and fails on a difference (ADR 0202), and checks the licence of every dependency (ADR 0801).
- A test that repeats an action identifies each result separately. An invitation already consumed by the test is not returned as the result of the next invitation, even while the recipient's event stream still holds it. Receiving a resolution on the sender does not prove that the recipient has applied it. The window invitation test interrupts the recipient stream to check this order.
- The PostgreSQL and LiveKit test wrappers print container state and timestamped logs into the test output before removing a failed run's containers. Failure to read those logs does not replace the test's exit status or prevent cleanup. Wrapper tests exercise these failure paths without starting containers.
- A window test waits for a state in real time, and the clock of the test of gpui moves with it: each wait of the harness (`Window::until`, `Window::until_read`) lets real time pass with `Window::pass`, which advances the clock of the test by the time that passed. The clock moves only while the scheduler parks, and the waits run the ready tasks without parking; before, a timer of the window (`background_executor().timer`) ended only when another call of the test happened to move the clock, and a state behind such a timer arrived sometimes and sometimes not within the wait of 60 s. A wait that ends without its state states how far the clock of the test moved.
- A press of the harness (`Window::click` and every press at a point) waits until the requests to a server that the press started have an answer: the client core counts the requests of a client that are on their way (`Client::requests`, built only with `test-support`), from the call of the API on the thread of its caller, before the request goes to a task of the runtime of the client core, so that a request that the press asked for is counted before the wait looks; and the wait leaves out exactly the requests that a `FaultProxy` of the same test holds (the request, or its answer): each request has a number from one counter of the process, which the task of the runtime carries to the transport and the transport sends as the header `Junto-Test-Request`, and the proxy records the numbers that it holds. A command that the window sends to the stream task of a server (open the messages of a place, read a page, read a collection of the administration again, send a message) is counted the same way where it is sent: it takes a number, and the stream task runs the reads and sends that it starts for the command, at the command and at its next step, with that number, and keeps it until it gave the readers the state with their answers; a read that the stream task queues behind another and starts later is not counted for the press. A request that the client core makes by itself, such as a read of its event stream, is counted at the transport; so a hold from before the press or of another client does not end the wait for an unheld request of the press. A test that holds a request on purpose still presses, and the press settles the window once more after the last answer. Before, a press ran only the queued work of the window, and a test that read the state just after a press depended on the speed of the server. A press of the primary button on a named element (`Window::click` and the like) also fails the test when the element is not drawn any more in the frame that follows the pointer going down, with a message that names it: gpui then has no element to end the press on and drops it without a sign, and a test failed later at a place that did not name the press. It fails in the same way when gpui refused the press while the element is still drawn, because the element moved and something else came to where it was, or the release was on a part of it that is not shown: gpui counts those presses under its test platform (`gpui::refused_presses`, ADR 0757, point 6). A press whose element only moved is delivered by gpui and is not reported. A press of the secondary button is not checked: the client opens its menus when that button goes down, and the menu can take the place of the element (a row of the hover menu of a server tile, a member of the group details). A test that presses a control which a read already on its way can take away waits for that read first (`Window::until_answered`). `Window::press_as_drawn` keeps pressing without a wait, for the tests of a press on what was drawn before a change; `Window::press_down_at` and `Window::release_at` press and release apart, for the tests of a change between them. The harness does not move the pointer to the control before a press: with that move, `admin::an_administrator_drags_channels_to_another_place_and_category` (the drag does not start) and `clock_format::messages_notices_and_quiet_hour_entry_use_the_chosen_clock` (the press on a time field gives the focus to the dialog and not to the field) fail at every run, for a cause in the hover of the press that is not found yet.
- Administration window tests perform role changes, overrides, retention settings,
  workspace redemption and selection, invite grants and revocation, request approvals,
  and token revocation through drawn controls. They check the visible result and the
  state read through the server API. A scoped test recorder observes the text and control
  labels at paint time, delegates text layout and paint to gpui, and keeps no records
  outside its scope. Tests check role names, switch words, selected-choice labels and
  retention values from those paints; role order and removed rows use drawn element
  bounds. The export tests compare the complete saved messages with the channel
  and thread messages read independently through the API, and check attachments,
  recording files and transcripts. The archive warning runs server maintenance with the test clock, and the recordings page reads
  enough stored rows to exercise its older-page control and event refresh. A window
  test saves a file and the transcript from a row of the recordings page and compares
  the file with the file of the server.
- A test of the media engine that requires the absence of a tone in what a device played examines everything that the device played since the step began, and only after the device has received that tone. The synthetic sound host keeps every played sample after `SyntheticHost::record_played` (until the next clear), in place of the last 12 seconds, so the examination does not depend on how long the step took; before, the tests required that the step took less than 10 seconds, which a host under load passed. `Person::until_receiving` in `crates/media/tests/calls` waits until the engine has subscribed to the microphone track of the other person and received its frames for at least 300 of the last 600 ms; before, a device that had not yet subscribed played no tone and passed the check without its sound block being tested. Tests: `hidden::a_place_that_moves_to_another_device_in_a_hidden_call_keeps_its_rules` and the two other absence checks of a sound block in `hidden.rs` and `share.rs`.
- A test that checks that an action waits for a limit does not check a window of its own inside that limit when the limit is short: `calls::ordering_tests::the_end_of_a_call_waits_for_the_work_of_a_move_within_its_limit` checks a 200 ms window only against the limit of 60 s, and the limit of 500 ms by the time that the end took.
- The signal proxy of the media tests passes the end of a connection that the media server closed while its answers were held, after those answers, when the test passes them; before, the client never saw that end. Test: `signal_proxy::the_end_of_a_connection_whose_answers_wait_follows_the_answers`.
- The gpui test platform has an in-memory clipboard. The invite-code and token tests
  seed it with another value, click Copy, compare the copied text, use that text against
  the server, and check the copied confirmation and Done. Integration with the system
  clipboard remains a native-system check.

Tests on real systems:

- An explicitly selected native macOS tier verifies virtual audio, camera and visible
  screen capture through the production implementations (ADR 0913). It is separate
  from ordinary and ignored CI tests and requires operator-managed permissions.

- The rehearsal installs a deployment with `docs/install.md` ("Trial on one machine") and
  runs two windows of the application through its first use: owner code, invite,
  messages, a thread, a call, shares, a recording with known speech, the saved files,
  the transcript, search, and a second start on both profiles. The steps are in
  `crates/client/tests/app/rehearsal.rs`. Its share-stop step opens the current
  multi-share menu and chooses Stop all shares, then checks that the own capture and
  the other window's received share end.
  - On macOS the test target `rehearsal` runs them in two real windows of the system on
    the main thread. The windows are drawn by Metal, and the picture of each step is read
    from the GPU with `Window::render_to_image`, without screen capture. A shared screen
    is checked by the colours that the receiving window draws.
  - The windows use gpui's `VisualTestAppContext`: real windows and rendering, with the
    tasks on gpui's test scheduler. The harness reaches them through a `TestAppContext`
    whose public application and executor fields are set to the native ones, because
    gpui gives the drawn positions of named elements only to its test contexts. The run
    loop and the dispatcher of the product are therefore not exercised, and the
    production entry point (`main.rs`, `app::state`) is not the one that runs.
  - Pointer events reach the window through `Window::dispatch_event`, the call that the
    input callback of the platform window makes. Text is typed one key at a time with
    `Window::dispatch_keystroke`, gpui's simulation of a key: it runs the key bindings and
    gives the text of the key to the input handler of the focused field. The key events of
    AppKit and the text input client of macOS are not exercised.
  - The checks read the view state from which the window draws (`ShellView` and the
    administration view) and the drawn bounds of named elements; the share check reads
    drawn pixels. One action is not made through the window: the download directory is
    set on the application state, so that saved files go into the work directory of the
    run and not into the Downloads folder of the user.
  - Sound, camera and screen are the synthetic devices of `junto-media`. The microphone
    of each window plays a tone, or queued speech from the speech generator of the
    transcription worker, so the real worker transcribes known words. Nothing opens a
    device, asks for a permission or plays a sound.
  - The steps that need no recorder also run against the server of the tests in the
    window live tier, on gpui's test platform, so that a change of the window that breaks
    a step fails there.

- On macOS the test target `frames` opens two real windows of the system in the same way,
  in a call with the server and the media server of the tests, with the synthetic shares
  and cameras of both people. It reads the graphics memory of the process with
  `footprint` while the windows draw the pictures of the call, and after the shares and
  the cameras end (ADR 0774).

- The test target `native_panics` runs the client on the run loop of the system, without
  the test runner and without a server, as the binary does. A task of the thread of the
  windows panics, and on macOS the end of a press that AppKit delivers panics; the client
  must continue, write each report and state it (ADR 0757). The macOS CI job runs it.

- On macOS the test target `endurance` runs a long use in the same way: windows A and B
  and a third person without a window in a call, A and B sharing, all three with a
  camera, the call of A in its separate window. The main window of A reads a channel of
  400 messages, scrolls up its history and back, opens and closes threads and changes
  channels in a loop for 10 minutes after 5 minutes of warming. Once in each cycle of
  the use (8 actions, about 20 seconds), after its last action, when the main window shows
  the end of the same channel with no thread open, it reads `phys_footprint` and the
  graphics memory with `footprint`, the bytes that the Rust allocations of the process
  hold, the processor time with `ps`, the open file descriptors and the threads. A missing
  graphics category in the output of `footprint` fails the run; it does not read as 0.
  The warming is 5 minutes: in a run with 90 seconds the footprint still grew by about
  9 MiB in the next 4 minutes and stayed level after that.
  - The bytes of the Rust allocations come from the global allocator of the test program,
    the crate `junto-alloc-count`: it passes every call to the allocator of the system and
    counts the bytes of the blocks that are held. The crate is a dependency of the tests
    only, and its implementation of `GlobalAlloc` is the one place of the crate where
    `unsafe` code is allowed. The footprint is not precise enough for a slow leak: it also
    changes with the free pages that the allocator of the system keeps and with the
    compressed pages, and the dirty memory of the regions of the allocator varied between
    349 and 428 MiB in one run, also when read at the same action of the cycle.
  - It measures an optimized build: the release profile with debug assertions
    (`CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS=true`), because the harness finds elements
    by the names that gpui keeps only with debug assertions. The build script states the
    optimization level, and the target refuses an unoptimized build: there the layout of
    gpui alone held the thread for up to 568 ms at a scroll, which is not a property of
    the product.
  - Growth is a rate: the slope of all readings after the warming, as the median of the
    slopes between every two readings (Theil–Sen). One collection of an allocator or a
    few readings far from the others do not move it. The run fails above 0.5 MiB a minute
    (Rust allocations), 3 MiB a minute (footprint), 0.5 MiB a minute (graphics), 2 files
    or 2 threads a minute. A run that held 1 MiB a minute more in Rust allocations
    measured 1.02 MiB a minute and failed. The run of the committed code, inside the machine lock with no other test container (load average 3.4 at the start and 4.3 at the end), passed: Rust allocations 0.01 MiB a minute, footprint 0.51 MiB a minute, graphics 0.00, 0.30 files and 0.12 threads a minute. The footprint of runs
    without a leak measured -2.93, -0.16, 0.34, 0.42, 0.57 and 1.25 MiB a minute, so its
    bound fails only a leak of native memory (Metal, the media engine) of several MiB a
    minute; a leak of the Rust code of 1 MiB a minute fails the bound of the Rust
    allocations. A leak of one file or one thread at each channel change is above the
    bounds of files and threads. The function is tested with such series in the `app`
    target.
  - The harness times every piece of work of the application on the thread of the
    windows: the work of one event of the pointer or the keyboard with the work that its
    handlers start (a click, a scroll), and one frame of the display with the work before
    and after it. The run fails when one piece after the warming took longer than the
    bound of ADR 0730. On gpui's test platform the background tasks of gpui run on the
    thread of the test, so a piece includes them; work on the runtime of the client and
    on the threads of the media engine is not included, as in the product.
  - Closing/reopening a main shell while its application state lives uses the same
    window harness. Its helpers create visual contexts, draw the first frame and pump
    tasks; the lifecycle tests do not bypass the source guard with raw context calls.

- On macOS the test target `modes` measures one window per process in seven modes of the
  use and prints the memory, the processor share and the threads of each (ADR 0914).

- The native targets run only with their own variable (`JUNTO_TEST_FRAMES=1`,
  `JUNTO_TEST_ENDURANCE=1`, `JUNTO_TEST_MODES=1`, the rehearsal variables): they run without the test runner and
  read no filter of test names, so the variables of the media server alone do not start
  them.

- Real capture of screen, sound and microphone is tested on each operating system:
  - a real macOS machine;
  - a real Linux desktop, in an X11 session first and then in a Wayland session;
  - a Windows 11 virtual machine. Its host needs hardware virtualization enabled in its firmware.
- The packaged application is installed and updated on a clean system of each kind.
- Real microphones and GPUs are tested on physical machines before a release.

## Consequences

- The recording path is tested from a published signal to the content of the output file, without a person.
- Native capture is checked separately from CI, with explicit device tests and operator-assisted checks, so an error in capture can pass CI.
- A release needs a macOS machine, a Linux desktop with both session types, a Windows 11 virtual machine and a clean system of each kind.
- Capture on Windows is tested in a virtual machine. Real microphones and GPUs on Windows are covered only by the physical-machine tests before a release.

The media engine’s external track-permission hold control is stored only with the `test-support` feature, as is its setter. The actor always owns the operative sender; production builds do not retain a second, unread test-control field. The count of the answers of the media server to publications of the camera, which only the camera check of the tests reads, is also compiled only with `test-support`.
