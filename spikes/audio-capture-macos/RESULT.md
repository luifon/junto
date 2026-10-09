# Spike: system and application sound capture on macOS

## Question

Can a Rust program on this Mac (macOS 26.6.1, Apple Silicon) capture (a) the
sound of one chosen application only and (b) all system sound, each with the
capturing program's own sound excluded, and deliver PCM samples at a known
format in real time?

**Answer: yes, with ScreenCaptureKit.** All seven gates were observed with the
capture program started from a terminal application that already holds the
"Screen & System Audio Recording" permission. One configuration is not
verified and needs a person: the capture program running as its own
application bundle with its own permission (see G5 and "Blocked").

| Gate | Result |
|---|---|
| G1 application-only capture | PASS (observed) |
| G2 all-system capture | PASS (observed) |
| G3 own sound excluded | PASS (observed), with two limits described below |
| G4 format and latency | PASS (observed): 48 kHz, 2 channels, float32, 20 ms buffers, about 90 ms |
| G5 permission behaviour | observed without permission; the grant step is blocked: needs a person |
| G6 capture at volume 0 or muted | PASS (observed): level is identical at volume 0, at volume 0 unmuted, and muted |
| G7 minimum version, window sharing | macOS 13.0; a window filter delivers the owning application's sound (observed) |

Core Audio process taps were not tested, because ScreenCaptureKit met every
gate. Two findings below are cases where the taps would be the alternative.

## Method

- `src/` is one Rust binary, `spike`, with these commands: `list`, `preflight`,
  `request`, `wav`, `tone`, `capture`.
- API: Apple ScreenCaptureKit through the crate `screencapturekit` 11.0.0
  (licence MIT OR Apache-2.0). Tone playback uses `cpal` 0.18.2 (Apache-2.0).
  Every dependency for the macOS target has an MIT, Apache-2.0, Zlib or
  BSD-2-Clause licence option (`cargo tree --format "{p} | {l}"`, observed).
- Each sound source is a sine tone of amplitude 0.2 (-14.0 dBFS) at its own
  frequency. `capture` records the delivered PCM, mixes the channels to mono,
  and measures the amplitude at each frequency with a Goertzel filter over the
  whole capture with a Hann window (`src/analysis.rs`). A tone is PRESENT above
  -30 dBFS and ABSENT below -50 dBFS. Nothing was judged by listening.
- Sources (`run.sh`, function `start_players`):

  | Frequency | Source |
  |---|---|
  | 1000 Hz | application `WindowedA`: own bundle, has a window, started with `open` |
  | 2500 Hz | application `WindowedB`: own bundle, has a window, started with `open` |
  | 3200 Hz | application `Windowless`: own bundle, no window, started with `open` |
  | 7300 Hz | `afplay` as a child process of application `ChildHost`, started with `open` |
  | 6100 Hz | `afplay` started from the same terminal as the capture process |
  | 4300 Hz | the capture process itself (`--own-tone`) |
  | 5300 Hz | nothing (reference) |

  `make_apps.sh` builds the four throwaway bundles under `out/apps`. The two
  windowed ones contain a copy of the installed `ffplay` binary; `Windowless`
  contains a copy of `spike`.
- A browser on this Mac was playing sound during the tests (observed with
  `diagnostics/audio_processes.swift`: one browser helper process with
  `running_output=1`). It is part of "all system sound", so it appears in the
  system captures as broadband content of about -16 dBFS RMS and -65 to -95
  dBFS at the test frequencies. Two extra captures exclude that browser to
  show the levels without it.
- Sound: `quiet.sh` reads the output volume and mute state, sets volume 0 and
  muted, runs the command, and restores the previous state on exit. Every
  capture ran under it. The state before and after every run was
  `output volume:88, output muted:false` (observed).

## Commands

```sh
../../tools/serial.sh cargo build --release
./make_apps.sh
BACKGROUND_BUNDLE=<browser bundle id> ./quiet.sh ./run.sh baseline matrix siblings
./quiet.sh ./run.sh volume_0_muted
QUIET_VOLUME=0 QUIET_MUTED=false ./quiet.sh ./run.sh volume_0_unmuted
QUIET_VOLUME=5 QUIET_MUTED=true  ./quiet.sh ./run.sh volume_5_muted
./quiet.sh ./run.sh formats
BACKGROUND_BUNDLE=<browser bundle id> ./quiet.sh ./run.sh latency
./target/release/spike preflight
open -n -g -W --stdout "$PWD/out/p.out" out/apps/CaptureApp.app --args preflight
open -n -g -W --stdout "$PWD/out/r.out" out/apps/CaptureApp.app --args request --wait 40
```

## API calls and filter configuration

Stream configuration used for every capture (`src/capture.rs`, `run`):

```
SCStreamConfiguration
  width = 16, height = 16, minimumFrameInterval = 1 s   (video is not used)
  capturesAudio = true
  sampleRate = 48000
  channelCount = 2
  excludesCurrentProcessAudio = true
SCStream(filter:configuration:delegate:)
stream.addStreamOutput(handler, type: .audio, sampleHandlerQueue: queue)
stream.startCapture()
```

| Mode | Filter |
|---|---|
| One application | `SCContentFilter(display: d, including: [application], exceptingWindows: [])` |
| All system sound | `SCContentFilter(display: d, excludingWindows: [])` |
| All system sound except some applications | `SCContentFilter(display: d, excludingApplications: [applications], exceptingWindows: [])` |
| One window | `SCContentFilter(desktopIndependentWindow: window)` |

`d` is the first entry of `SCShareableContent.displays`. `application` is the
`SCRunningApplication` found in `SCShareableContent.applications` by bundle
identifier or process identifier. In the crate these are
`SCContentFilter::create().with_display(..)` followed by
`.with_including_applications(&[&app], &[])`, `.with_excluding_windows(&[])`,
`.with_excluding_applications(&apps, &[])`, or `.with_window(&window)`.

## Observed levels (dBFS at each frequency)

All rows: output volume 0, muted. `-200.0` means exact digital silence.

| capture | 1000 Hz | 2500 Hz | 3200 Hz | 4300 Hz | 6100 Hz | 7300 Hz | 5300 Hz | RMS |
|---|---|---|---|---|---|---|---|---|
| `baseline_no_players` (system) | -65.7 | -72.3 | -78.8 | -93.9 | -73.7 | -92.8 | -74.8 | -16.1 |
| `app_a` (application WindowedA, exclude self) | -14.0 | -200.0 | -200.0 | -200.0 | -200.0 | -200.0 | -200.0 | -17.0 |
| `app_b` (application WindowedB, exclude self) | -167.6 | -14.0 | -165.1 | -174.6 | -182.7 | -185.2 | -179.9 | -17.0 |
| `app_a_include_self` (exclude self off) | -14.0 | -200.0 | -200.0 | -200.0 | -200.0 | -200.0 | -200.0 | -17.0 |
| `window_a` (window of WindowedA) | -14.0 | -168.8 | -173.8 | -178.9 | -185.4 | -188.1 | -182.7 | -17.0 |
| `system_exclude_self` | -14.0 | -14.0 | -14.0 | -69.7 | -86.5 | -14.0 | -83.5 | -9.8 |
| `system_include_self` (control) | -14.0 | -14.0 | -14.0 | -14.0 | -14.0 | -14.0 | -70.5 | -8.2 |
| `system_exclude_self_without_background` | -14.0 | -14.0 | -14.0 | -167.0 | -171.4 | -14.0 | -174.4 | -11.0 |
| `system_include_self_without_background` (control) | -14.0 | -14.0 | -14.0 | -14.0 | -14.0 | -14.0 | -140.5 | -9.3 |
| `siblings_app` (see G3, limit 2) | -14.0 | -135.7 | -132.3 | -14.0 | -14.0 | -144.2 | -133.4 | -12.3 |

Representative raw output (`app_a`):

```
filter=SCContentFilter(display:includingApplications:[application]:exceptingWindows:[]) application_pid=65296 bundle="spike.audiocapture.WindowedA"
configuration capturesAudio=true sampleRate=48000 channelCount=2 excludesCurrentProcessAudio=true
format sample_rate=48000 channels=2 bits_per_channel=32 bytes_per_frame=4 format_flags=0x29 float=true big_endian=false subtype="lpcm" buffers_in_list=2 channels_per_buffer=1
callback_thread name="<unnamed>" id=ThreadId(2)
audio_buffers=215 frames_per_buffer_min=960 frames_per_buffer_max=960 buffer_ms=20.00
captured_frames=206400 captured_seconds=4.300 rms_dbfs=-17.0
tone hz=1000 level_dbfs=-14.0 verdict=PRESENT
tone hz=2500 level_dbfs=-200.0 verdict=ABSENT
```

## Gates

### G1. Application-only capture: PASS (observed)

Capture of `WindowedA` contains 1000 Hz at -14.0 dBFS and 2500 Hz at -200.0
dBFS. Capture of `WindowedB` contains 2500 Hz at -14.0 dBFS and 1000 Hz at
-167.6 dBFS. The browser sound is also absent from both (RMS -17.0 dBFS is
exactly one tone of amplitude 0.2).

Findings about what "one application" means (all observed):

- The application's sound includes the sound of its helper and child
  processes. A capture of the browser application (main process) had RMS
  -16.5 dBFS while only its helper process, with a different bundle
  identifier, was producing sound.
- `SCShareableContent.applications` lists only processes that have a
  connection to the window server. `WindowedA` and a terminal-started `ffplay`
  with a window are listed. `afplay`, `ffplay -nodisp`, `spike tone` and the
  bundled `Windowless` application are not listed, so ScreenCaptureKit cannot
  select them as the one application. They are captured in the all-system
  mode (3200 Hz and 7300 Hz at -14.0 dBFS).

### G2. All-system capture: PASS (observed)

`system_exclude_self` contains 1000, 2500, 3200 and 7300 Hz at -14.0 dBFS
each, and the browser sound.

### G3. Own sound excluded: PASS (observed), with two limits

- All-system mode, `excludesCurrentProcessAudio = true`: the capture process's
  4300 Hz tone is at -69.7 dBFS, which is the level of the browser sound at
  that frequency, and at -167.0 dBFS when the browser is excluded. Control
  with `excludesCurrentProcessAudio = false`: -14.0 dBFS. The flag therefore
  removes a tone that is otherwise captured at full level.
- Application mode: the own tone is at -200.0 dBFS with the flag on and with
  the flag off, because the capture process is not in the filter.

Limit 1 (observed): the exclusion covers more than the capture process. The
6100 Hz `afplay`, started from the same terminal as the capture process, is
also removed (-86.5 dBFS, -171.4 dBFS without the browser; -14.0 dBFS in the
control). The 7300 Hz `afplay`, which also has an empty bundle identifier but
is a child of a different application, is kept. The exclusion therefore
follows the application that macOS holds responsible for the process, and its
descendants. For a bundled application this means its own process and its
child processes; this exact case is not verified (see "Blocked").

Limit 2 (observed, `siblings_app`): when the application in an including
filter belongs to the same responsible application as the capture process,
the capture contains the whole group, the capture process's own tone
included (4300 Hz at -14.0 dBFS with `excludesCurrentProcessAudio = true`).
The flag has no effect in that case. The product must not offer itself in the
application list.

### G4. Format and latency: PASS (observed)

- Format: linear PCM, 48 000 Hz, 2 channels, 32-bit float, native endian,
  non-interleaved (`mFormatFlags = 0x29`: float, packed, non-interleaved). The
  `AudioBufferList` holds one buffer per channel.
- Buffer size: 960 frames (20.00 ms) in every buffer of every capture.
- Other requests (`run.sh formats`): 16 000 Hz mono, 24 000 Hz stereo and
  8 000 Hz mono are delivered as requested, always in 20 ms buffers. A request
  for 44 100 Hz is delivered as 48 000 Hz, so the format must be read from the
  sample buffer.
- Latency (`run.sh latency`, 5 rounds): time from the source's output callback
  that holds the first tone sample to the capture callback that delivers that
  sample: 91.2, 86.7, 102.2, 85.0, 97.9 ms (mean 92.6 ms). The source reports
  13.7 ms device output latency and the sample is 8.0 ms into its buffer, so
  the capture callback runs about 71 ms after the moment the sample would
  leave the speakers.
- At each capture callback, host clock minus the buffer's presentation
  timestamp: mean 55 to 60 ms, minimum 42 ms, typical maximum 70 to 90 ms.
  Single buffers were later: up to 224 ms in the recorded runs, and 342 and
  562 ms in two earlier runs, each in the first capture after new players
  started.
- Start-up: `startCapture` returned after 131 to 413 ms; the first audio
  buffer arrived 143 to 487 ms after the call.

### G5. Permission behaviour: observed without permission; grant step blocked

- Permission: "Screen & System Audio Recording" in System Settings, Privacy &
  Security.
- The permission belongs to the application that macOS holds responsible for
  the process, not to the binary. The same `spike` binary:
  - started from the terminal: `CGPreflightScreenCaptureAccess=true`, all
    captures work, no prompt appeared at any time (observed);
  - copied into `CaptureApp.app` and started with `open`:
    `CGPreflightScreenCaptureAccess=false` (observed).
- Without the permission the result is an error, not silence (observed):

  ```
  pid=88041 CGPreflightScreenCaptureAccess=false
  CGRequestScreenCaptureAccess=false
  SCShareableContent error: NoShareableContent("Content unavailable: The user declined TCCs for application, window, display capture")
  after 40 s CGPreflightScreenCaptureAccess=false
  ```

  `SCShareableContent` fails at once, so no filter and no stream can be made.
- macOS shows a dialog for the application, read through the accessibility
  tree (observed): title "Screen Recording", text "“CaptureApp” would like to
  record this computer’s screen and audio. Grant access to this application in
  Privacy & Security settings, located in System Settings.", buttons "Open
  System Settings", "Deny", "Help". There is no button that allows the access
  with one click. The dialog appeared again on a later launch.
  `CGPreflightScreenCaptureAccess` shows no dialog.
- Blocked: needs a person. Granting requires a change in System Settings.
  The test did not make that change, so these are not verified: whether the switch
  asks for a password or Touch ID, whether the application must be restarted
  after the grant, and every gate with the capture program as its own
  application.
- Not verified: Core Audio process taps use a different permission. The same
  settings pane has a second list, "System Audio Recording Only" (observed,
  empty on this Mac).

### G6. Capture at volume 0 or muted: PASS (observed)

| capture | 1000 Hz | RMS | output state |
|---|---|---|---|
| `volume_0_muted` (application) | -14.0 | -17.0 | volume 0, muted |
| `volume_0_unmuted` (application) | -14.0 | -17.0 | volume 0, not muted |
| `volume_5_muted` (application) | -14.0 | -17.0 | volume 5, muted |
| the same three in all-system mode | -13.9, -14.0, -14.0 | | |

The captured level equals the source amplitude (0.2 = -14.0 dBFS) in every
state. The capture does not depend on the output volume or the mute state, so
a person who shares can keep the speakers silent. No audible volume was used
at any time.

### G7. Minimum macOS version and window sharing

- `SCStreamConfiguration.capturesAudio`, `sampleRate`, `channelCount`,
  `excludesCurrentProcessAudio` and `SCStreamOutputType.audio` are marked
  `API_AVAILABLE(macos(13.0))` in the macOS 26.5 SDK headers (observed in
  `SCStream.h`). ScreenCaptureKit itself is macOS 12.3. Minimum for this
  design: **macOS 13.0**. Behaviour on versions before 26 is not verified.
- Window sharing: the API has no audio setting per window.
  `SCContentFilter(desktopIndependentWindow:)` with `capturesAudio = true`
  delivers the sound of the application that owns the window and nothing else
  (observed, `window_a`: 1000 Hz at -14.0 dBFS, all other tones below -168
  dBFS). Not verified: an application with several windows; the API gives no
  way to separate them.

## Problems hit

- The crate `screencapturekit` compiles a Swift bridge at build time. It needs
  the Swift toolchain on the build machine, and the binary failed to start
  with `Library not loaded: @rpath/libswift_Concurrency.dylib` until `build.rs`
  added `-Wl,-rpath,/usr/lib/swift`.
- `SCContentFilter(desktopIndependentWindow:)` aborted a command-line process
  with `Assertion failed: (did_initialize), function CGS_REQUIRE_INIT`. Calling
  `CGMainDisplayID()` first fixes it. A process that already runs an
  application event loop is not expected to hit this (not verified).
- All test processes started from one terminal count as one application for
  the audio filters. The first test run, with such players, gave misleading
  results (application mode contained all three tones, all-system mode with
  the exclusion contained none). The players had to be separate bundles
  started with `open`.
- A browser was playing sound during the tests. Absence in the all-system mode
  is therefore shown twice: against the browser's level and with the browser
  excluded.
- 4000 Hz and 6000 Hz were first used and replaced, because the windowed
  player's 16-bit output puts harmonics of 1000 Hz at about -106 dBFS.

## What the product needs

- A signed application bundle with a stable code identity. The permission is
  attached to the responsible application; an ad hoc signed bundle changes
  identity with every build.
- Check `CGPreflightScreenCaptureAccess()` before use; it shows no dialog.
  Handle the `SCShareableContent` error, and send the person to System
  Settings, because the system dialog has no allow button.
- Enumerate with `SCShareableContent` (`applications`, `windows`,
  `displays`). Remove the product's own application from the list (G3,
  limit 2). Applications without a window are not in the list.
- One `SCStream` gives video and audio. For sharing, add a `.screen` output
  and an `.audio` output to the same stream; the sound mode is chosen by the
  filter: display filter for all system sound, including-applications filter
  or window filter for one application.
- Keep alive for the whole capture: the `SCStream`, the output handler object
  and its dispatch queue. Stopping is `stopCapture`. A change of mode can use
  `updateContentFilter` on the running stream (present in the API, not
  tested).
- Samples arrive on a dispatch queue thread, not on the main thread and not on
  a real-time audio thread (observed: unnamed thread, the crate's serial
  queue; a custom queue can be passed). Copy the samples into a ring buffer
  for the LiveKit audio source there and return.
- Read the format from each `CMSampleBuffer` (`CMAudioFormatDescription`).
  Expect float32, non-interleaved, one buffer per channel, 20 ms. Request
  48 000 Hz stereo; convert to the format the LiveKit audio source takes
  (interleaved 16-bit in the Rust SDK; not verified here).
- Use the buffer's presentation timestamp to align sound with video frames of
  the same stream. Plan for about 90 ms from playback to delivery.
- Set `excludesCurrentProcessAudio = true` always. Play the call's sound from
  the product's own process or a child process, so it is in the excluded
  group.
- Decision for the binding: `screencapturekit` works but adds a Swift build
  step and a run-time search path. `objc2-screen-capture-kit` 0.3.2 (Zlib OR
  Apache-2.0 OR MIT) calls the same Objective-C API without Swift; it was not
  tested here.

## Blocked: needs a person

1. In System Settings, Privacy & Security, Screen & System Audio Recording,
   allow `CaptureApp` after running `./make_apps.sh` and one
   `open out/apps/CaptureApp.app --args request`, then run the matrix with the
   capture program as that application. This verifies G3 and G5 in the
   product's configuration.
2. That pane now has one row named `CaptureApp` with its switch on (observed
   after the tests). The test did not turn a switch on: the only screen action
   on a permission control was one click on the dialog's "Deny" button, and
   that command returned an error. The pane was not read before the tests, so
   it is not known whether the row existed before. The test application reported
   `CGPreflightScreenCaptureAccess=false` in two launches after the row was
   seen, so the row does not give this build access. Remove the row if it is not wanted (select it and press the minus
   button).

## Left in place

- Source: `Cargo.toml`, `Cargo.lock`, `build.rs`, `src/`, `quiet.sh`,
  `run.sh`, `make_apps.sh`, `diagnostics/` (two Swift sources used for the
  process list and the dialog check), this file.
- Deleted: `target/`, `out/` (tone files, captured WAV files, logs, the
  throwaway bundles) and the compiled diagnostics in the temporary directory.
- Output volume and mute state are as they were before the tests.
