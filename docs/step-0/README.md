# Step 0: technical tests before product code

Step 0 is a set of eight technical tests. Product code starts after the result of every test is recorded in this file and in the affected ADRs ([ADR 0903](../adr/0903-step-0-and-build-order.md)).

Rules:

- Each test has a stated result that passes or fails.
- A failure changes the design or removes a feature from the first version.
- The results can remove a feature from the first version ([`../scope.md`](../scope.md)). They do not add to it.

How to record a result: tick the box of each check that passed, fill the result table of the test, and set the line "Result" to "passed" or "failed" with the date. When a result changes a decision, change the ADR in the same commit.

A check that did not run is marked "not run". A check that ran in part, or under conditions that limit its result, is marked "partly", and the limit is stated next to it. A test in which a check failed, did not run or ran in part has the result "partly passed", and the result names those checks. A check that did not run or ran in part stays open. The section [Open after step 0](#open-after-step-0) lists what is missing.

The results below were recorded on 2026-10-09. Each result links to the section of a `RESULT.md` file under [`../../spikes/`](../../spikes/README.md) that holds the method, the commands and the observed output. All test sound and all test pictures were generated signals: tones, patterns and synthetic speech. No test used a recording of a person.

## Summary

| Test | Subject | Decides or tests | Result |
|---|---|---|---|
| 1 | Sound capture | Which sound choices each system offers (ADR 0502) | Partly passed: macOS and Linux passed, Windows not run |
| 2 | gpui | The dependency revisions; how much a screen reader can use (ADR 0701, ADR 0702) | Partly passed: one dependency set on macOS and Linux, file drop failed on X11, Windows not run |
| 3 | Recorder | The layout page, the separate audio files, recovery of the job state (ADR 0003, ADR 0601, ADR 0602) | Partly passed: layout page and audio files passed, recovery from the job list failed and changed the design |
| 4 | macOS update | Where device keys are stored on macOS; permissions after an update (ADR 0103, ADR 0704) | Partly passed: both permissions remain, the key store failed and the key file is used |
| 5 | Network | Calls over TURN on port 443 only; behaviour when the link between the hosts drops (ADR 0004, ADR 0506) | Partly passed: link loss recorded, TURN over TLS on port 443 not run |
| 6 | Capacity | The capacity of the media host (ADR 0003) | Partly passed: load points with a recording measured, not with TURN and the HTTPS entry |
| 7 | Diarization component | The diarization component (ADR 0603) | Passed on synthetic voices; real voices not tested |
| 8 | Packaged install | Installation on a clean system of each kind (ADR 0705) | Not run |

## Test 1: Sound capture

Subject: capture of sound for a screen share on each of the three systems.

Checks, on each system:

- One application alone is captured.
- All system sound is captured.
- The sound of the Junto application itself is excluded in both.

| System | One application alone | All system sound | Own sound excluded (one application) | Own sound excluded (all system sound) |
|---|---|---|---|---|
| macOS | [x] passed, for applications that have a window | [x] passed | [x] passed, with one limit | [x] passed |
| Windows 11 | [ ] not run | [ ] not run | [ ] not run | [ ] not run |
| Linux with PipeWire | [x] passed | [x] passed | [x] passed by the selection rule | [x] passed |

Gate:

- A sound choice passes on a system when the capture contains the intended source and does not contain the application's own sound.
- The result decides which choices each system offers. A choice that fails on a system is not offered on that system.
- Screen share with sound is required on all three systems in the first version (ADR 0502). A system on which no choice passes fails the test, and the failure changes the design or removes the feature from the first version.

Result: partly passed, 2026-10-09. macOS and Linux passed every check. Windows was not run, because no Windows machine was available.

Notes:

Every source was a sine tone at -14.0 dBFS with its own frequency. A capture was judged by the measured level at each frequency, not by listening.

macOS, on an Apple Silicon machine with macOS 26.6.1, with ScreenCaptureKit ([`spikes/audio-capture-macos/RESULT.md`][mac]):

- One application: the chosen application's tone at -14.0 dBFS, the other application's tone at -167.6 dBFS or digital silence ([G1][mac-g1]).
- All system sound: the tones of all four other sources at -14.0 dBFS ([G2][mac-g2]).
- Own sound, all system sound: with the exclusion on, the capture program's tone was at -167.0 dBFS. While another application played broadband sound it was at -69.7 dBFS, which is the level of that sound at the tone's frequency. In the control without the exclusion it was at -14.0 dBFS ([G3][mac-g3]).
- Own sound, one application: the capture program's tone was digital silence, because the program is not in the filter ([G3][mac-g3]).
- Limit of "one application": the system lists only applications that have a window. A program without a window is captured only in the all-system choice. The capture of an application includes the sound of its helper and child processes ([G1][mac-g1]).
- Limit of the exclusion: it covers the application that macOS holds responsible for the capturing process, and that application's child processes. When the chosen application belongs to the same responsible application as the capturing process, the capture contains the program's own tone at -14.0 dBFS ([G3][mac-g3]).
- The captured level is the same at output volume 0, at volume 0 without mute, and muted ([G6][mac-g6]).
- Format: 48 kHz, 2 channels, 32-bit float, buffers of 20 ms. Delay from playback to delivery: 85 to 102 ms, mean 92.6 ms ([G4][mac-g4]).
- Minimum version: the audio settings of the API are available from macOS 13.0, by the SDK headers. No version before 26 was run ([G7][mac-g7]).
- Condition of every capture: the capture program was started from a terminal application that already held the "Screen & System Audio Recording" permission. The same program as its own application bundle, without the permission, got an error and no samples. The grant for that bundle was not made, so no capture ran with the permission of the program's own bundle ([G5][mac-g5]).

Linux, on an x86-64 desktop with PipeWire 1.6.8 and WirePlumber 0.5.15 in an X11 session ([`spikes/audio-capture-linux/RESULT.md`][lin]):

- One application: the chosen process's tone at -14.0 dBFS, the tones of two other processes at -128 dBFS or lower. The three processes were a native PipeWire client, a PulseAudio client and an ALSA client ([L1][lin-l1]).
- All system sound: both tones at -14.0 dBFS. The capture program's own tone was at -160 dBFS, the floor of the analysis, while a recording of the same output device had it at -14.0 dBFS. A stream that started 5 s after the capture was linked 13 ms after it appeared ([L2][lin-l2]).
- Own sound, one application: the capture links only the nodes of the chosen application, so its own nodes are not linked. The program's own tone was not played in that run ([L1][lin-l1]).
- Identity of an application: the process identifier that a client reports is the identifier inside its sandbox, and two sandboxed applications both reported 2. A rule that used it excluded the wrong application. A node identifier was reused for a later stream ([How an application is identified][lin-id]).
- Loopback nodes: with a forwarding node in the graph, the all-system capture contained the tone twice (-8.0 dBFS). With nodes that have the property `node.link-group` skipped, it was -14.0 dBFS ([Other observations][lin-other]).
- Format: 32-bit float, 48 kHz, 2 channels. The block size was 256, 1024 and 2048 frames (5.3 to 42.7 ms) and changed during a run. Samples arrive in the graph cycle in which the application wrote them ([L4][lin-l4]).
- In 1 of 8 starts of a new stream, the first block (42.7 ms) was not captured ([Other observations][lin-other]).
- A client that PipeWire classified as a Flatpak sandbox captured correctly. A real Flatpak build, an AppImage build, a Wayland session, JACK clients and other PipeWire versions were not run ([L5][lin-l5], [L6][lin-l6]).

## Test 2: gpui

Subject: the functions that the client needs from gpui, on each system, with one fixed set of dependency revisions.

Checks, on each system:

| Check | macOS | Windows 11 | Linux X11 | Linux Wayland |
|---|---|---|---|---|
| Text input, including an input method | [ ] partly | [ ] not run | [ ] partly | [ ] partly |
| Text selection | [x] passed | [ ] not run | [x] passed | [ ] partly |
| File drop | [ ] not run | [ ] not run | [ ] failed | [ ] not run |
| A second window | [x] passed | [ ] not run | [x] passed | [x] passed |
| Operating-system notifications | [x] passed | [ ] not run | [x] passed | [ ] not run |
| Interface scaling | [x] passed | [ ] not run | [x] passed | [ ] partly |
| Several video tiles | [x] passed | [ ] not run | [ ] partly | [ ] partly |
| What a screen reader reads (recorded below) | [x] recorded | [ ] not run | [x] recorded | [ ] not run |

Dependency revisions:

- [ ] One set of revisions of gpui and of the component library works together on all systems. Partly: one set builds and runs on macOS and on Linux (X11 and Wayland) from one lock file. Windows was not run.

| Dependency | Revision |
|---|---|
| gpui | `gpui-pre` 0.3.8 from crates.io, with `gpui-pre-platform` 0.3.8 and the platform crates of the same version. It is a snapshot of the gpui sources; the platform crate records the source revision `279fe070bb389b79652e52065b2f001edcc0b11b`. |
| Component library | `gpui-component` 0.7.1 from crates.io, with `gpui-base` 0.7.1 and `gpui-kit-assets` 0.7.1. It requires `gpui-pre` at exactly 0.3.8. |

Gate:

- Each check other than the screen reader passes when the function works on that system with the fixed set of revisions.
- The screen reader check has no pass condition. Its result is a description of what a screen reader reads on each system, and it decides how much a screen reader can use in the first version (ADR 0702).
- The dependency check passes when one set of revisions is fixed. If no set works, the result is the decision to fork the component library (ADR 0701).
- A function that fails on a system changes the design or removes the feature that depends on it from the first version.

Result: partly passed, 2026-10-09. One dependency set is fixed for macOS and Linux. The file drop failed on X11 and did not run on the other systems. Windows was not run.

Notes:

Test machines: an Apple Silicon machine with macOS 26.6.1 and a 120 Hz display ([`spikes/gpui-client/RESULT.md`][gm]), and an x86-64 Linux desktop with an NVIDIA GPU, in an X11 session and under a headless Wayland compositor in a container ([`spikes/gpui-client-linux/RESULT.md`][gl]). Two conditions limit the Linux frame rates: the X11 session had no monitor and no direct rendering, so each finished frame was copied to the X server through its socket; and other tests used the machine at the same time (1-minute load average 2.5 to 25).

Dependency set ([macOS C1][gm-c1], [Linux C1][gl-c1]):

- All crates come from crates.io and are Apache-2.0. No package in the lock file is GPL-only.
- The `gpui-pre` crates are published by the maintainer of the component library, not by the maintainers of gpui. A new snapshot was published each week during the test period.
- The lock file made on macOS did not work on Linux. The lock file made with the Linux features `x11` and `wayland` has 870 packages and works on both systems.
- A set built from git revisions of the gpui repository and of the component library was not tried.
- Linux build: Rust 1.98.1 and the development files of fontconfig, libxcb, libxkbcommon and libxkbcommon-x11. The binary ran on the desktop without an installed package.
- Size: binary 32.9 MB on macOS and 66.5 MB on Linux. Idle window: 88 MB resident memory on macOS, 237 MB on Linux in the X11 session with the NVIDIA driver, of which 173 MB are mapped driver libraries.

Text input ([macOS C2][gm-c2], [Linux C2][gl-c2]):

- Passed on macOS, X11 and Wayland: typing, Enter and Shift+Enter, paste of several lines, word wrap, growth to six rows, undo, selection by keyboard, dead keys.
- macOS: the system emoji picker opened and inserted a character. The press-and-hold accent menu was not verified, because the test tools could not open it.
- Linux: no system emoji picker exists. A selection is not written to the primary selection, and a middle click does not paste. The Compose key was observed under Wayland and not verified on X11.
- A full input method (for example Japanese or Chinese) was not run on any system. No input method was running in the Linux session.

Text selection ([macOS C3][gm-c3], [Linux C3][gl-c3]): a selection across lines and across several messages copies on macOS and X11. Author names and times are not part of a selection. Under Wayland the selection by keyboard and the copy were observed; the selection by mouse drag was not, because the pointer tool could not hold a button.

File drop ([macOS C7][gm-c7], [Linux C7][gl-c7]):

- X11: failed. A drag of two files from a GTK window onto the drop area ended without a call of the drop handler, in 3 of 3 runs. The drag source reported that it delivered the file addresses, and gpui logged the drop event. The cause was not found. No file manager was running in the session, and no second drag source was available.
- macOS: not run. The test tools could not drive a system drag session; a control window that accepts file drops received nothing either.
- Wayland: not run. The pointer tool could not hold a button.
- The drag of a person to another voice channel inside the application passed on macOS and X11, and the same move through a menu passed.

Second window ([macOS C8][gm-c8], [Linux C8][gl-c8]): a second window opens, both windows update from one shared state, and the window closes by its button and by the close control, on macOS, X11 and Wayland. Keeping it above other windows:

- macOS: the floating window kind gives a higher window level. The kind is set when the window is created; gpui has no call that changes it later.
- X11: the floating window kind keeps the window above the main window only. A window of another program is above it. After the standard window-manager request for the state "above", the window stayed above the other program; gpui has no call that sends this request.
- Wayland: after a click on the main window, the floating window was behind it. Observed on one compositor.

Notifications ([macOS C10][gm-c10], [Linux C10][gl-c10]):

- macOS: title, body and two actions were delivered, and the click on an action and on the body came back to the application. The application must run from an application bundle, and macOS asks for a permission at the first notification. No banner was seen on screen, because a screen recording was active and macOS hides banners in that state; the notification was in Notification Center.
- X11: a banner with two actions was shown, and both clicks came back. A second notification with the same tag does not replace the first, and the call that dismisses a notification does nothing.

Interface scaling ([macOS C6][gm-c6], [Linux C6][gl-c6]): 90 % to 150 % at run time on macOS and X11, through the rem size of the window. Sizes that are given in pixels do not scale: a dialog kept its width of 448 pixels at 150 % while its text grew. At 150 % in a window 1320 points wide, the three side columns left 180 points for the content. The desktop scale of 150 % was observed on X11 and on Wayland. The interface scale itself was not run under Wayland.

Video tiles ([macOS C9][gm-c9], [Linux C9][gl-c9]), tiles of 1280×720 at 30 frames per second:

| System and path | 1 tile | 4 tiles | 9 tiles | Condition |
|---|---|---|---|---|
| macOS, surface path | 30.0 fps, 7 % of one core | 30.0 fps, 9 % | 30.0 fps, 10 % | |
| macOS, image path | 30.0 fps, 12 % | 30.0 fps, 22 % | 30.0 fps, 43 % | |
| Linux, image path, headless Wayland compositor on the GPU | 30.0 fps, 30 to 32 % | 21.6 to 24.3 fps, 107 to 108 % | 11.1 to 11.4 fps, 143 to 166 % | no real display; load average 10 to 11 |
| Linux, image path, X11 session | 10.5 fps | 8.5 fps | 6.2 fps | each frame copied through the X socket; not a measurement of a desktop |
| Linux, image path, software rendering | 30.0 fps, 624 % | 20.0 fps, 505 % | 11.7 fps, 388 % | load average 8 to 11 |

- Linux has no surface path: it exists only on macOS.
- With tiles of 640×360, the headless Wayland configuration showed 9 tiles at 30.0 fps with 53 % of one core (load average 5 to 6).
- No Linux configuration was a display driven by the GPU. The frame rates on a Linux desktop with a real display were not measured.

Scrolling of a list of 20,000 messages ([macOS C4][gm-c4], [Linux C4][gl-c4]): frame interval 8.33 ms on macOS (120 Hz). On Linux the interval was 40 to 152 ms in the three configurations above, none of which is a display driven by the GPU. The draw time per frame, which does not depend on the display, was 3.35 ms (median) on macOS and about 11 ms on the Linux desktop at low load.

What a screen reader reads ([macOS C11][gm-c11], [Linux C11][gl-c11]):

- The accessibility tree was read through the accessibility API of macOS and through AT-SPI on Linux. No screen reader program was started on either system.
- In the tree, with label and state: buttons, switches, check boxes, text inputs, headings with text, lists with items, menus and menu items. An activation from the accessibility side runs the click handler.
- Not in the tree: message text (the Markdown text view adds nothing to the tree), plain text children of elements without a role, and a focusable element without a role.
- Linux: the same nodes as on macOS. gpui publishes the tree only while the accessibility status of the session is enabled.
- Result: a person who depends on a screen reader can operate the labelled controls and cannot read a conversation, unless the client adds an accessible node for each message.

Keyboard ([macOS C12][gm-c12], [Linux C12][gl-c12]): Tab order, focus ring, a shortcut bound to an action, and two keyboard paths to the hover menu passed on macOS and X11. On Linux a binding must name the secondary modifier, which is Ctrl there.

Secret storage on Linux ([L-B][gl-lb]): with a secret service running in the session, a secret was stored, read and deleted in 0.115 s without a prompt. A locked collection, a session without a secret service, and the key file were not run.

Packaging facts on Linux ([L-C][gl-lc]): the binary needs three libraries that a package can bundle (libxcb, libxkbcommon, libxkbcommon-x11). The C library, the graphics driver and the session services come from the host. A build on Debian 12 needs glibc 2.35 or newer on the host. No package was built.

## Test 3: Recorder

Subject: the recorder on the data host, connected to the media server on the public host (the two-host form of ADR 0003).

Checks:

- [x] The recorder loads the project's own layout page, and the video shows the main share with its sound. Passed, with one required property of the page.
- [x] The separate audio files are correct across one reconnect of a participant. Passed.
- [x] The separate audio files are correct across one mute of a participant. Passed.
- [ ] After the recorder restarts, the job state is recovered. Failed for the design as it was: the recorder's job list reports the jobs of a lost recorder as active without end. The design changed (ADR 0601, ADR 0602).

Gate:

- The layout check passes when the MP4 shows the most recently started share as the main picture and contains the sound of that share and the microphones (ADR 0602).
- The audio file checks pass when each audio track has its own file, a reconnect produces a new file with its own start time, and the files fit one timeline (ADR 0602).
- The recovery check passes when the server has the correct state of the recording after the recorder restarts (ADR 0601).
- A failure changes the recording design or removes a recording feature from the first version.

Result: partly passed, 2026-10-09. The layout page and the audio files passed, on one machine and in the two-host form. The recovery check failed and changed the recording design. No recording feature is removed.

Notes:

Two runs: all components on one arm64 machine ([`spikes/recorder/RESULT.md`][r1]), and the two-host form with the recorder on an x86-64 data host that cannot receive connections, LiveKit and Redis on the public host, and a private link between them ([`spikes/recorder-two-hosts/RESULT.md`][r2]). Versions: LiveKit server 1.13.9, LiveKit Egress 1.15.0. The participants were processes of the synthetic participant in [`spikes/test-participant/`][tp]. Other work used both test machines during most runs; each number below states its condition where the result file states one.

Layout page ([R1][r1-r1], [X2][r2-x2], [R6][r1-r6]):

- The main share's tone was at -12.0 dBFS and the tone of every other share below -78 dBFS, with all microphone tones at -12 dBFS. Picture and sound changed to a third share when it started. In the two-host form this held in 7 of 7 calls at 1080p.
- Required property: a page that subscribed to every track kept playing the old share's tone at -12 dBFS after the main share changed (three runs). The page that subscribes only to the tracks it plays did not.
- The server can name the main share in the start request and with a layout update on a running recording. The page switched 0.1 to 0.4 s after the request.
- With 16 cameras, the page as tested subscribed to every camera at full quality. Every publisher then sent its highest layer, and the recorder used its whole limit of 6 CPUs. With the lowest layer requested for the tiles, the recorder used 2.6 CPUs ([X7][r2-x7]).

Audio files ([R2][r1-r2], [R3][r1-r3]):

- One Ogg file with Opus per audio track, each with its own tone at -12.0 dBFS and every other tone at -83 dBFS or lower.
- The recorder encodes again: every file is stereo Opus at about 128 kbit/s, also with the pass-through setting. No tested request stores the packets as they were sent.
- Reconnect: the file of the old track ended complete 0.20 s after the participant left. The new tracks have new identifiers, and the controller must start a new file for each. With a poll once per second, the new files missed the first 0.71 and 0.77 s.
- Mute of 20.0 s: the same file continues, with 20.008 s of silence and no gap in its timestamps.
- Not run: a mute much longer than 20 s, and a moderator's microphone block.

Timeline ([Alignment][r1-align], [X6][r2-x6]):

- Track files: all 24 measured files are 41 to 56 ms late against their reported start time. With 47 ms subtracted, the error is -6 to +9 ms.
- Composite: its content begins 1.49 to 1.96 s before the start time that the recorder reports, and the value differs from recording to recording. Placed by its end time and its media length, the error is -15 to +26 ms in 9 recordings.
- Once, on the loaded data host, the delay of the composite's sound changed inside a recording: from 1.60 s to 2.40 s for one participant and to 1.90 s for another, and back to 1.63 s over the next 60 s.
- Not run: recordings longer than 4 minutes, so drift over an hour is not known.

Recorder restart and crash ([R4][r1-r4], [X3][r2-x3], [X4][r2-x4]):

- After a restart or a kill of the recorder, its job list kept reporting every job as active, with no end time and no error. This was still the case 35 minutes later.
- With MP4 output, nothing playable survived a kill: the partial file has no index. With the composite as segments of 2 s, every finished segment survived, and the loss was 4.16 and 4.49 s at 1080p. The surviving segments were joined into one MP4 without encoding again in 0.11 to 0.30 s.
- The partial Ogg files of the audio tracks play. They are in the recorder's temporary directory, which the image deletes at every start. With that directory on a mounted volume and a start script that saves the files first, the loss was 1.0 to 3.0 s.
- Signals of a lost recorder: its participant left the room's participant list 19.4 to 21.6 s after the kill; a stop request answered with a timeout after 3.0 s; its health port did not answer at once.
- With a restart policy for the recorder, a new recording in the same room was requested 2.3 s after a crash and recorded. The composite then has a gap of about 9 s.
- A restart of the LiveKit server lost only the seconds of the restart: the audio files ended complete and new ones were started for the new tracks, and the composite continued with 10 s without sound.
- A recorder that receives a stop signal finishes its recordings when it is given the time. The default restart of the container did not give it the time.

Result check ([X8][r2-x8]): a recorder limited to 0.5 CPU for 25 s reported a complete recording with no error, and the file had 21.7 s of holes in its sound. A check of packet timestamps, digital silence and key frames found all 6 defective files and passed every good file. It took 3.3 to 4.0 s for a file of 130 s and 80 s for a file of 61 minutes that was made by repeating one recording.

Recorder cost ([R5][r1-r5], [X7][r2-x7]):

| Recording | Recorder CPU, mean / highest sample | Recorder memory | Condition |
|---|---|---|---|
| 1080p30 composite as segments, with 5 audio files | 2.5 cores / 2.9 | 1.36 GiB | x86-64 data host, quiet |
| 1080p30 composite as MP4, with 5 audio files | 2.6 cores / 4.8 | 1.42 GiB | x86-64 data host, quiet |
| 1080p30 composite as segments, with 5 audio files | 3.9 cores / 5.4 | 1.55 GiB | x86-64 data host, loaded by other work |
| 720p30 composite as MP4, with 4 audio files | 0.94 core / 2.2 | 0.62 to 0.67 GiB | arm64 machine, loaded by other work |
| One audio file | 0.04 to 0.08 core | 82 to 88 MB | the recorder's own figure |

- With the recorder's default CPU costs per request, the second audio file of a recording started about 15 s late (three calls). With lower configured costs, start requests returned in 0.12 to 0.8 s.
- File sizes: a 1080p30 composite 43.4 MB for 132 s as MP4 and 6.0 to 6.5 % more as segments; an audio file about 0.97 MB per minute.

Private link and two hosts ([X1][r2-x1], [X5][r2-x5]):

- The link is WireGuard in userspace, in one container per host, opened from the data host. Redis answered through it in 13 ms on average and was not reachable in any other way from outside the public host.
- No kernel module is needed. The first version of the link container made the kernel load its WireGuard module on both hosts without using it, because it asked for the interface before the userspace process was ready. The corrected start script waits for the control socket, and a full recording passed with no module loaded. Every other result was measured with the unused module loaded ([Deviation][r2-dev]).
- A port that Docker publishes was reachable before any firewall rule for it existed.
- Link loss of 30 s: see test 5.

Not run in this test: webhooks, a Redis restart during a recording, TLS between the recorder and the LiveKit server, real screen content and speech, and the storage guard.

## Test 4: macOS update

Subject: an update of an application that is signed with the project's own self-signed certificate.

Checks, after the update:

- [ ] The device key in the system key store is readable. Failed: the read needs a prompt with the password of the login keychain.
- [x] The microphone permission remains. Passed.
- [x] The screen-recording permission remains. Passed.

Gate:

- The test passes when all three checks pass.
- If the device key is not readable, macOS stores the device key in a key file that only the user can read (ADR 0103).
- If a permission does not remain, macOS asks for it again after each update, and the install instructions state this (ADR 0704).

Result: partly passed, 2026-10-09. Two of three checks passed. By the gate, macOS stores the device key in a key file (ADR 0103). Both permissions remain after an update when every build is signed with the same self-signed certificate (ADR 0704).

Notes:

On an Apple Silicon machine with macOS 26.6.1 ([`spikes/macos-identity/RESULT.md`][id]). Three builds of one application were installed one after the other, once with an ad-hoc signature and once signed with a self-signed code-signing certificate.

| Property after an update | Ad-hoc signature | Self-signed certificate |
|---|---|---|
| Designated requirement ([I1][id-i1]) | changes: it is the code hash of each build | stable: bundle identifier and hash of the certificate |
| Key store item of the first build readable without a prompt ([I2][id-i2]) | no | no |
| Microphone permission ([I3][id-i3]) | lost, new prompt | remains in the second and third build |
| Screen-recording permission ([I4][id-i4]) | lost, new prompt | remains in the second and third build |
| Data Protection keychain ([I5][id-i5]) | refused, missing entitlement | refused, missing entitlement |
| Key file with mode 0600 readable ([I6][id-i6]) | yes | yes |
| First launch of a downloaded copy ([I7][id-i7]) | blocked | blocked |

- Key store: each item records the code hash of the build that created it. An updated build is refused whatever its signature is, and macOS then asks for the password of the login keychain. The certificate changes nothing here: the read failed with the certificate present and absent.
- Permissions: macOS stores the designated requirement with each permission. With the self-signed certificate it is the pair of bundle identifier and certificate hash, so it is the same for every build.
- The certificate is not needed on the user's machine. The permissions remained with the certificate absent from the machine at every step ([I8][id-i8]).
- A certificate whose validity period had ended could not be used to sign ([Extra][id-extra]).
- A downloaded copy is blocked at the first launch with both signatures. The dialog has no button that opens the application; System Settings then offers "Open Anyway". That button was not clicked.
- Only the permission status and the preflight call were read. No microphone or screen capture ran after an update.
- Not run: a build used after its certificate expired, a standard (non-administrator) account, a build with the hardened runtime, macOS versions other than 26.6.1.

## Test 5: Network

Subject: the network paths for calls.

Checks:

- [ ] A call works when the network allows only TURN over TLS on port 443. Not run.
- [x] The behaviour of calls when the link between the hosts drops is recorded below.
- [ ] The behaviour of admission checks when the link between the hosts drops, and when it returns, is recorded below. Partly: the behaviour of the LiveKit API during the outage is recorded; no admission check ran.

Gate:

- The first check passes when a call works on a network that allows only TURN over TLS on port 443 (ADR 0004). A failure changes the design of the network paths.
- The second and third checks record behaviour and have no pass condition of their own. The recorded behaviour is compared with ADR 0506, which states that no admission check runs while the link is down and that the server compares the participants with the people allowed when its connection to LiveKit returns. A difference changes that ADR.

Result: partly passed, 2026-10-09. The first check was not run: the configuration for TLS on port 443 with three server names and TURN over TLS is written in [`spikes/media-host/tls-plan/`](../../spikes/media-host/tls-plan/NOTES.md) and was not applied. The behaviour at a link loss is recorded.

Notes:

Media server on a host with one CPU core: LiveKit server 1.13.9 and Redis 7.2.16 in containers ([`spikes/media-host/RESULT.md`][mh]).

Paths without TLS ([M3][mh-m3]):

- UDP on one port: a call between two clients worked, with 0 packets lost. Clients used IPv6 for media when they had it.
- TCP port, with all outbound UDP blocked at the client: a client based on libwebrtc connected over TCP and exchanged audio and video with 0 packets lost in 30 s. The command-line tool based on the LiveKit Go SDK did not connect.

Link between the hosts down for 30 s ([X5][r2-x5]); the data host was loaded by other work during this run:

- Calls: they continued. A subscriber received audio and video of every publisher in every second of the outage.
- Recordings: they continued, and every file is complete. The recorder reaches the LiveKit server on its public address and uses the link only for Redis.
- LiveKit API on the public address: the participant list and the job list answered normally during the outage.
- Requests that pass through Redis to the recorder: a stop request answered with a timeout after 3.0 s. A start request waited and returned 3 s after the link came back.
- State after the outage: a recording file that ended during the outage was reported as active until 16 s after the link returned.
- The link came back by itself 0.61 s after its container started. No container was restarted and no request was repeated.
- Not run: an admission check, API calls through the link, an outage longer than 30 s, and an outage of the path while both link containers keep running.

Redis stopped for 29 s during a call ([M6][mh-m6]): both clients lost the call 6 s after Redis stopped. New joins and the server API failed. After Redis returned, one of two clients reconnected by itself; the other had ended its attempts after 28 s and had to join again. Media was missing for about 25 s.

Token lifetime ([M7][mh-m7]):

- A token with a lifetime of 60 s was accepted 54 s after its expiry and refused 65 s after it.
- The LiveKit server sends a connected client a new token with a lifetime of 600 s, whatever the lifetime of the join token.
- A participant who was removed through the API joined again with the original token, and 80 s after the removal with the token of 600 s.
- A removal therefore keeps a person out only when the check is repeated for as long as the room exists.

Other facts ([M1][mh-m1], [What the product's deployment needs][mh-needs]): LiveKit warns at start that the UDP receive buffer of the host is too small and asks for 5,000,000 bytes; with the default, the host counted UDP send-buffer errors under load. With host networking, the metrics port of LiveKit listens on all addresses.

## Test 6: Capacity

Subject: the capacity of the media host, measured with test participants while a recording runs.

Checks:

- [ ] The measurement ran with test participants and a running recording. Partly: two load points and one small call ran with a recording; the other load points ran without one.
- [x] The measured limit is recorded below.

All figures are from a host with one CPU core and 4 GB of memory. The core is a virtual CPU. "Media server CPU" is the share of that one core, as average and highest sample. "Loss" is the figure of the load generator.

| Participants | Cameras | Screen shares | Recording running | Result |
|---|---|---|---|---|
| 5 connections: 3 publish, 2 subscribe | 3 | 2 | no | Media server CPU 11.9 % / 28 % |
| the same | 3 | 2 | yes, 1080p30 composite and 5 audio files | 9.4 % / 13 % and 10.3 % / 18 % |
| 1 publisher, 5 subscribers | 0 | 1 stream of 2.5 Mbit/s | no | 3 % / 5 %; loss 0.07 % |
| 1 publisher, 20 subscribers | 0 | 1 | no | 7 % / 9 %; loss 0.06 % |
| 1 publisher, 40 subscribers | 0 | 1 | no | 14 % / 30 %, loss 0.12 %; in two later runs 23.0 % / 43 % and 28.0 % / 32 %, loss 1.25 % and 4.75 % |
| 1 publisher, 40 subscribers | 0 | 1 | yes, 1080p30 composite | 28.0 % / 55 % and 25.7 % / 36 %; loss 3.14 % and 1.23 % |
| 1 publisher, 80 subscribers | 0 | 1 | no | 22 to 28 %; loss 0.33 % |
| 1 publisher, 120 subscribers | 0 | 1 | no | 36 to 42 %; loss 1.7 %; the limit was the network of the load generator |
| 8 publishers, 8 subscribers | 8 with simulcast | 0 | no | 13 % / 20 %; loss 0.01 % |
| 16 publishers, 16 subscribers | 16 with simulcast | 0 | no | 23 % / 37 %, loss 2.36 %; in a later run 28.8 % / 57 %, loss 4.70 % |
| 16 publishers, 16 subscribers | 16 with simulcast | 0 | yes, layout page as tested | 31.9 % / 44 %; loss 2.76 %; the recorder used its whole limit of 6 CPUs |
| 20 publishers, 20 subscribers, audio only | 0 | 0 | no | 35 % / 50 %; loss 0 % |
| 50 publishers, 50 subscribers, audio only, 25 streams per subscriber | 0 | 0 | no | 70 to 80 %; loss 0.27 %; the limit was the CPU of the media server |

Gate:

- The test passes when a call of the size that the tested deployment is intended for, with one screen share, runs on the media host while a recording runs. The intended size and the measured limit are recorded with the result.
- A failure changes the deployment form of ADR 0003.

Result: partly passed, 2026-10-09. Every load point with a recording ran, and the media server used at most 32 % of the core on average in them. This file states no size of a deployment; an intended call is compared with the load points of the table. The measured limit of the media server is the audio-only point with 50 participants. The deployment form of ADR 0003 does not change.

Notes:

Results: [M5][mh-m5] without a recording, [X7][r2-x7] with a recording.

- A recording did not raise the CPU of the media server by more than the difference between two runs of the same kind. The recorder ran on the data host.
- Conditions that limit the figures: the hypervisor took 1 to 16 % of the core on average in most runs and 51 % in one sample of 5 s. In the later runs the load generator reported 1.2 to 4.8 % loss without a recording too, and the place of that loss was not determined. The figures with a recording must be compared with the later runs without one.
- In the load tool a publisher does not subscribe, so N participants are 2N connections. The simulcast runs did not send the 720p layer.
- Audio costs more CPU per bit than video: the cost follows the number of packets and of connections. While 100 connections joined in about 20 s, the host had 17 to 19 % idle time and counted 9,131 UDP send-buffer errors.
- Memory of the media server: 27 MiB idle, 461 MiB at most. Redis: 3 to 4 MiB.
- Not measured: the load with TURN relaying, with the HTTPS entry on the same core, with every subscriber on the 720p layer, and with audio-file recordings at the load points.

## Test 7: Diarization component

Subject: the optional component that separates the voices inside shared sound.

Checks, for the chosen component:

- [x] It runs without network access. Passed.
- [x] Its licence terms allow distribution with the stated attribution. Passed.
- [x] Its result on a recording of several voices is usable. Passed on synthetic voices; not run on real voices.

| Component | Version | Licence | Runs without network | Result on several voices |
|---|---|---|---|---|
| Speaker embeddings from sherpa-onnx with the model `nemo_en_titanet_large.onnx`, on speech spans from Silero VAD v6, with agglomerative clustering | sherpa-onnx 1.13.8 | Apache-2.0 (code), CC-BY-4.0 (model, with attribution), MIT (voice detector) | Yes, in a container without a network | Synthetic voices: the right number of speakers on 8 of 8 files, 96 to 97 % of speech time under the right speaker, diarization error rate 13 to 16 % (2 to 3 % with a collar of 0.25 s), the same after two Opus encodes. Overlapping speech gets one speaker. The threshold is not verified on real voices. |

Gate:

- The test passes when one component passes all three checks. That component is chosen (ADR 0603).
- If no component passes, the design changes or speaker separation inside shared sound is removed from the first version.

Result: passed on synthetic voices, 2026-10-09. The component of the table is chosen (ADR 0603, ADR 0614). A check on real recordings was not run.

Notes:

On an x86-64 host with 8 cores and an NVIDIA GPU with 8 GB, in containers without a network ([`spikes/transcription/RESULT.md`][tr]). Other tests loaded the machine: every CPU speed below is a lower bound. All speech was synthetic, so the error rates compare settings and do not predict the accuracy on real meetings.

The test also covered the rest of the transcription chain:

- Components ([T1][tr-t1]): voice detection with Silero VAD v6, transcription with faster-whisper 1.2.1 on CTranslate2 4.8.2 and the model `large-v3-turbo`, speaker separation as in the table. All code is MIT or Apache-2.0. Every model file is a plain HTTPS download without an account. The pyannote pipelines were not usable, because their model files need an account.
- Voice detection ([T2][tr-t2]): without it, Whisper invented 19 to 220 lines for 10 minutes of noise with two sentences. With clips that the worker cuts itself, the transcript was exactly the two sentences. The built-in filter of the library lost 1 of 2 sentences.
- Language ([T3][tr-t3]): with one language detection per clip, the word error rate on mixed English and Portuguese was 0.9 to 1.4 %. In every built-in mode it was 49 to 52 %, because one language was left out. English and Portuguese alone: 0.3 to 1.5 %.
- Glossary ([T4][tr-t4]): terms spelled right rose from 3 of 23 to 21 of 23 (English) and from 7 of 23 to 19 of 23 (Portuguese). The library cuts the list at 223 tokens without a message.
- Speaker separation ([T5][tr-t5]): at a threshold of 0.7 instead of 0.6, two voices became one speaker and the error rate rose from 13 % to 36 % or more.
- Offline ([T6][tr-t6]): the chain made no socket call for an Internet address. ONNX Runtime 1.29 and 1.30 contain a telemetry client that is on by default; version 1.28.0 does not.
- Speed and memory ([T7][tr-t7]): with the GPU, 19 to 25 times real time for dense speech with a language per clip, 2.2 GB of memory and 3.3 GB of video memory. On the CPU with 8 threads, 0.2 times real time in the same mode and 0.8 to 1.0 times with speech joined into pieces of 30 s, on the loaded machine. Speaker separation ran at 17 to 25 times real time on the CPU.

## Test 8: Packaged install

Subject: installation of the packaged application on a clean system of each kind.

Checks:

- [ ] macOS 26 on Apple Silicon. Not run.
- [ ] Windows 11 on x86-64. Not run.
- [ ] Linux on x86-64, tarball. Not run.
- [ ] Linux on x86-64, AppImage. Not run.

Gate:

- A check passes when the packaged application installs and starts on a clean system of that kind, by the steps of the install instructions (ADR 0704, ADR 0705).
- A failure changes the packaging or removes the system or the package from the first version.

Result: not run. No package exists yet.

Notes:

Facts from other tests that the packaging uses: the first launch of a downloaded copy on macOS is blocked until the user allows it in System Settings (test 4); the libraries that a Linux package can bundle and those that come from the host (test 2).

## Decisions that follow from step 0

Accounts and signing:

- macOS stores the device key in a key file that only the user can read, not in the system key store (ADR 0103).
- Linux stores the device key in the secret service when one is running, and in the same kind of key file otherwise (ADR 0103).
- Every macOS build is signed with the project's own self-signed certificate; the bundle identifier and the certificate never change (ADR 0704).

Calls:

- macOS offers the sound of one application that has a window, all system sound, or none; it needs macOS 13 or later (ADR 0502).
- Linux offers the sound of one application that plays sound, all system sound, or none, by links in the PipeWire graph (ADR 0502).
- The Linux capture identifies an application without its process identifier and skips loopback nodes (ADR 0502).
- The admission check runs for as long as a room exists, because a removal does not end the validity of a token (ADR 0506).
- The admission check skips the participants of the recorder (ADR 0506).

Deployment:

- Redis is part of the call path and runs beside the media server with a restart policy and a health check (ADR 0003).
- The private link is WireGuard in userspace in a container on each host; it carries Redis for the recorder (ADR 0003).
- The recorder reaches the LiveKit server on its public address, so a loss of the link does not interrupt a recording (ADR 0003).
- The media host gets larger UDP buffers (ADR 0003).
- The TCP fallback is established for the libwebrtc-based client only (ADR 0004).

Recording:

- The layout page subscribes only to the tracks it plays, and the server names the main share (ADR 0602).
- The composite is recorded as segments of 2 s and joined into one MP4 after the stop (ADR 0602).
- Audio tracks are recorded as Ogg files, with the recorder's temporary directory on a mounted volume (ADR 0602).
- The files are placed on one timeline by the reported start of each audio file and by the end of the composite (ADR 0602).
- Every finished file is checked for missing sound and picture (ADR 0602).
- A lost recorder is detected from the participant list and a stop request, not from the job list (ADR 0601).
- The recorder is stopped only after its recordings ended (ADR 0601, ADR 0005).
- A recording reserves disk space for the segments and the MP4 together (ADR 0006).

Transcripts:

- The worker uses Silero VAD v6, faster-whisper with `large-v3-turbo`, and speaker embeddings with clustering (ADR 0603, ADR 0610, ADR 0614).
- The worker cuts the speech clips itself, and Whisper detects the language of each clip (ADR 0603, ADR 0613).
- The glossary is cut by the worker at 223 tokens in order of priority (ADR 0603, ADR 0613).
- ONNX Runtime is pinned to 1.28.0 and its telemetry setting is off (ADR 0007, ADR 0610).

Client:

- The client uses `gpui-pre` 0.3.8 and `gpui-component` 0.7.1 from crates.io, with one lock file for all systems (ADR 0701).
- On Linux the client requests a lower video resolution for small tiles (ADR 0701).
- Screen reader support stays outside the promise of the first version: labelled controls reach the accessibility tree, message text does not (ADR 0702).
- Sizes in the interface are given in rems, because sizes in pixels do not scale (ADR 0703).
- Attaching a file does not depend on a file drop (ADR 0703).
- "Keep on top" is set when the call window is created on macOS, needs a window-manager request on X11, and is not offered on Wayland (ADR 0507).

## Open after step 0

Not run:

| Open item | What it blocks |
|---|---|
| Every test on Windows: sound capture, gpui, the credential store, the packaged install | The Windows client. The sound choices of Windows (ADR 0502), the dependency set for all three systems (ADR 0701, ADR 0702), key storage on Windows (ADR 0103), the package (ADR 0705). |
| A call on a network that allows only TLS on port 443, with three server names and TURN over TLS (test 5, first check). The configuration is written and was not applied. | The network design of ADR 0004 and the public entry of the two-host form (ADR 0003). The TCP fallback for clients behind a network that allows only port 443. |
| The packaged install on a clean system of each kind (test 8) | The packages and the install instructions (ADR 0704, ADR 0705). |
| Sound capture and screen reading on macOS with the permissions of the product's own application bundle; real capture after an update | The macOS sound choices as the product ships them (ADR 0502) and the statement that capture continues after an update (ADR 0704). |
| Frame rates on a Linux desktop whose display is driven by the GPU | Any statement about the frame rate on Linux, and the tile sizes of ADR 0701. |
| A file drop from a file manager, on every system. The drop failed on X11 with the one drag source that was available. | The file drag of ADR 0703. Attaching by a file chooser is not blocked. |
| An admission check during and after a loss of the link, and API calls through the link (test 5, third check) | The known limit of ADR 0506 for the two-host form. |
| Capacity with TURN relaying and the HTTPS entry on the media host | The statement that one host carries the media server, the entry and TURN (ADR 0003). |
| A check of speaker separation and of the clip rules on real recordings | A release of speaker separation with the measured threshold (ADR 0614, ADR 0613). |

Found and not resolved:

| Finding | What it blocks |
|---|---|
| The recorder sends requests to public STUN servers of third parties by default. The setting that turns this off is not verified. | The rule of ADR 0007 that no component contacts a third party. |
| The recorder's debug log contains the room token of the recorder. | Nothing, when the log level stays above debug (ADR 0003). |
| A full input method, a screen reader program, and a locked secret service were not run on any system. | The text input for languages that need an input method (ADR 0701), the statement of ADR 0702 beyond the accessibility tree, the prompt case of ADR 0103. |
| Recordings longer than 4 minutes were not made. | The timeline constants over a long recording (ADR 0602). |
| The licence of the NVIDIA libraries in a built GPU image of the worker is not verified. | Distribution of a built GPU image (ADR 0610). |

[mac]: ../../spikes/audio-capture-macos/RESULT.md
[mac-g1]: ../../spikes/audio-capture-macos/RESULT.md#g1-application-only-capture-pass-observed
[mac-g2]: ../../spikes/audio-capture-macos/RESULT.md#g2-all-system-capture-pass-observed
[mac-g3]: ../../spikes/audio-capture-macos/RESULT.md#g3-own-sound-excluded-pass-observed-with-two-limits
[mac-g4]: ../../spikes/audio-capture-macos/RESULT.md#g4-format-and-latency-pass-observed
[mac-g5]: ../../spikes/audio-capture-macos/RESULT.md#g5-permission-behaviour-observed-without-permission-grant-step-blocked
[mac-g6]: ../../spikes/audio-capture-macos/RESULT.md#g6-capture-at-volume-0-or-muted-pass-observed
[mac-g7]: ../../spikes/audio-capture-macos/RESULT.md#g7-minimum-macos-version-and-window-sharing
[lin]: ../../spikes/audio-capture-linux/RESULT.md
[lin-l1]: ../../spikes/audio-capture-linux/RESULT.md#l1-app-only-capture-pass-observed
[lin-l2]: ../../spikes/audio-capture-linux/RESULT.md#l2-all-system-sound-without-own-sound-pass-observed
[lin-l4]: ../../spikes/audio-capture-linux/RESULT.md#l4-format-and-latency-pass-observed
[lin-l5]: ../../spikes/audio-capture-linux/RESULT.md#l5-what-the-method-requires-pass-observed-parts-reasoned
[lin-l6]: ../../spikes/audio-capture-linux/RESULT.md#l6-sandboxes-pass-for-a-simulated-flatpak-sandbox-observed-real-builds-not-verified
[lin-id]: ../../spikes/audio-capture-linux/RESULT.md#how-an-application-is-identified
[lin-other]: ../../spikes/audio-capture-linux/RESULT.md#other-observations
[gm]: ../../spikes/gpui-client/RESULT.md
[gm-c1]: ../../spikes/gpui-client/RESULT.md#dependency-set-c1
[gm-c2]: ../../spikes/gpui-client/RESULT.md#c2-composer--partial
[gm-c3]: ../../spikes/gpui-client/RESULT.md#c3-rich-text--pass
[gm-c4]: ../../spikes/gpui-client/RESULT.md#c4-long-list--pass
[gm-c6]: ../../spikes/gpui-client/RESULT.md#c6-interface-scale--pass
[gm-c7]: ../../spikes/gpui-client/RESULT.md#c7-drag-and-drop--partial
[gm-c8]: ../../spikes/gpui-client/RESULT.md#c8-second-window--pass
[gm-c9]: ../../spikes/gpui-client/RESULT.md#c9-video-tiles--pass
[gm-c10]: ../../spikes/gpui-client/RESULT.md#c10-notifications--pass
[gm-c11]: ../../spikes/gpui-client/RESULT.md#c11-accessibility--recorded
[gm-c12]: ../../spikes/gpui-client/RESULT.md#c12-keyboard--pass
[gl]: ../../spikes/gpui-client-linux/RESULT.md
[gl-c1]: ../../spikes/gpui-client-linux/RESULT.md#c1-dependency-set-and-build--pass
[gl-c2]: ../../spikes/gpui-client-linux/RESULT.md#c2-composer--partial
[gl-c3]: ../../spikes/gpui-client-linux/RESULT.md#c3-rich-text--pass
[gl-c4]: ../../spikes/gpui-client-linux/RESULT.md#c4-long-list--partial
[gl-c6]: ../../spikes/gpui-client-linux/RESULT.md#c6-interface-scale--pass
[gl-c7]: ../../spikes/gpui-client-linux/RESULT.md#c7-drag-and-drop--partial
[gl-c8]: ../../spikes/gpui-client-linux/RESULT.md#c8-second-window--pass
[gl-c9]: ../../spikes/gpui-client-linux/RESULT.md#c9-video-tiles--partial
[gl-c10]: ../../spikes/gpui-client-linux/RESULT.md#c10-notifications--pass
[gl-c11]: ../../spikes/gpui-client-linux/RESULT.md#c11-accessibility--recorded
[gl-c12]: ../../spikes/gpui-client-linux/RESULT.md#c12-keyboard--pass
[gl-lb]: ../../spikes/gpui-client-linux/RESULT.md#l-b-secret-storage--pass
[gl-lc]: ../../spikes/gpui-client-linux/RESULT.md#l-c-packaging-facts
[r1]: ../../spikes/recorder/RESULT.md
[r1-r1]: ../../spikes/recorder/RESULT.md#r1-custom-layout-page-pass
[r1-r2]: ../../spikes/recorder/RESULT.md#r2-separate-audio-files-pass
[r1-r3]: ../../spikes/recorder/RESULT.md#r3-reconnect-and-mute-pass
[r1-align]: ../../spikes/recorder/RESULT.md#alignment-observed
[r1-r4]: ../../spikes/recorder/RESULT.md#r4-state-recovery-fail-for-an-egress-restart-pass-for-a-livekit-restart
[r1-r5]: ../../spikes/recorder/RESULT.md#r5-limits-measured
[r1-r6]: ../../spikes/recorder/RESULT.md#r6-control-by-api-pass
[r2]: ../../spikes/recorder-two-hosts/RESULT.md
[r2-dev]: ../../spikes/recorder-two-hosts/RESULT.md#deviation-from-the-limits-of-the-task-a-kernel-module-was-loaded
[r2-x1]: ../../spikes/recorder-two-hosts/RESULT.md#x1-private-link-in-containers-pass
[r2-x2]: ../../spikes/recorder-two-hosts/RESULT.md#x2-recording-across-hosts-pass
[r2-x3]: ../../spikes/recorder-two-hosts/RESULT.md#x3-surviving-a-recorder-crash-measured
[r2-x4]: ../../spikes/recorder-two-hosts/RESULT.md#x4-detecting-a-lost-recorder-pass
[r2-x5]: ../../spikes/recorder-two-hosts/RESULT.md#x5-link-loss-pass
[r2-x6]: ../../spikes/recorder-two-hosts/RESULT.md#x6-timeline-pass-with-a-method
[r2-x7]: ../../spikes/recorder-two-hosts/RESULT.md#x7-cost-measured
[r2-x8]: ../../spikes/recorder-two-hosts/RESULT.md#x8-silent-failure-check-pass
[tp]: ../../spikes/test-participant/README.md
[id]: ../../spikes/macos-identity/RESULT.md
[id-i1]: ../../spikes/macos-identity/RESULT.md#i1-designated-requirement
[id-i2]: ../../spikes/macos-identity/RESULT.md#i2-keychain-login-keychain-generic-password-item-created-by-v1
[id-i3]: ../../spikes/macos-identity/RESULT.md#i3-microphone
[id-i4]: ../../spikes/macos-identity/RESULT.md#i4-screen--system-audio-recording
[id-i5]: ../../spikes/macos-identity/RESULT.md#i5-data-protection-keychain
[id-i6]: ../../spikes/macos-identity/RESULT.md#i6-key-file
[id-i7]: ../../spikes/macos-identity/RESULT.md#i7-first-launch-of-a-quarantined-copy
[id-i8]: ../../spikes/macos-identity/RESULT.md#i8-certificate-on-the-users-machine
[id-extra]: ../../spikes/macos-identity/RESULT.md#extra-expired-certificate
[mh]: ../../spikes/media-host/RESULT.md
[mh-m1]: ../../spikes/media-host/RESULT.md#m1--stack-up-pass
[mh-m3]: ../../spikes/media-host/RESULT.md#m3--a-call-over-udp-and-over-tcp-pass
[mh-m5]: ../../spikes/media-host/RESULT.md#m5--capacity-measured
[mh-m6]: ../../spikes/media-host/RESULT.md#m6--redis-stops-during-a-call-observed
[mh-m7]: ../../spikes/media-host/RESULT.md#m7--token-lifetime-facts-observed
[mh-needs]: ../../spikes/media-host/RESULT.md#what-the-products-deployment-needs
[tr]: ../../spikes/transcription/RESULT.md
[tr-t1]: ../../spikes/transcription/RESULT.md#t1-component-choice--pass
[tr-t2]: ../../spikes/transcription/RESULT.md#t2-voice-detection-removes-invented-text--pass-with-the-workers-own-clips
[tr-t3]: ../../spikes/transcription/RESULT.md#t3-transcription-quality--pass
[tr-t4]: ../../spikes/transcription/RESULT.md#t4-glossary-effect--pass
[tr-t5]: ../../spikes/transcription/RESULT.md#t5-diarization--pass-on-synthetic-voices
[tr-t6]: ../../spikes/transcription/RESULT.md#t6-offline-and-no-telemetry--pass
[tr-t7]: ../../spikes/transcription/RESULT.md#t7-resources--pass-measured-a-server-without-a-gpu-is-slow
