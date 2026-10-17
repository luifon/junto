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

Tests on real systems:

- An explicitly selected native macOS tier verifies virtual audio, camera and visible
  screen capture through the production implementations (ADR 0913). It is separate
  from ordinary and ignored CI tests and requires operator-managed permissions.

- The rehearsal installs a deployment with `docs/install.md` ("Trial on one machine") and
  runs two windows of the application through its first use: owner code, invite,
  messages, a thread, a call, shares, a recording with known speech, the saved files,
  the transcript, search, and a second start on both profiles. The steps are in
  `crates/client/tests/app/rehearsal.rs`.
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

- On macOS the test target `endurance` runs a long use in the same way: windows A and B
  and a third person without a window in a call, A and B sharing, all three with a
  camera, the call of A in its separate window. The main window of A reads a channel of
  400 messages, scrolls up its history and back, opens and closes threads and changes
  channels in a loop for 10 minutes after 5 minutes of warming. Every 5 seconds it reads
  `phys_footprint` and the graphics memory with `footprint`, the processor time with
  `ps`, the open file descriptors and the threads. A missing graphics category in the
  output of `footprint` fails the run; it does not read as 0. The warming is 5 minutes:
  in a run with 90 seconds the footprint still grew by about 9 MiB in the next 4 minutes
  and stayed level after that.
  - It measures an optimized build: the release profile with debug assertions
    (`CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS=true`), because the harness finds elements
    by the names that gpui keeps only with debug assertions. The build script states the
    optimization level, and the target refuses an unoptimized build: there the layout of
    gpui alone held the thread for up to 568 ms at a scroll, which is not a property of
    the product.
  - Growth is a rate: the slope of all readings after the warming, as the median of the
    slopes between every two readings (Theil–Sen). One collection of an allocator or a
    few readings far from the others do not move it. The run fails above 1.5 MiB a minute
    (footprint), 0.5 MiB a minute (graphics), 2 files or 2 threads a minute. Runs measured
    -0.16 to 0.63 MiB a minute of footprint and up to 0.46 files a minute. A leak
    of 2 MiB a minute, or of one file or one thread at each channel change, is above
    these bounds; the function is tested with such series in the `app` target.
  - The harness times every piece of work of the application on the thread of the
    windows: the work of one event of the pointer or the keyboard with the work that its
    handlers start (a click, a scroll), and one frame of the display with the work before
    and after it. The run fails when one piece after the warming took longer than the
    bound of ADR 0730. On gpui's test platform the background tasks of gpui run on the
    thread of the test, so a piece includes them; work on the runtime of the client and
    on the threads of the media engine is not included, as in the product.

- The native targets run only with their own variable (`JUNTO_TEST_FRAMES=1`,
  `JUNTO_TEST_ENDURANCE=1`, the rehearsal variables): they run without the test runner and
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
