# Memory and processor time of the desktop client

This document states what the desktop client uses in each mode of its use on macOS, how that compares with other chat clients, and what was changed to use less. The method is in ADR 0914: the test target `modes` of `junto-client`, one process per mode, the other people of a call in separate processes, the release profile without debug assertions.

## Junto, per mode

Measured on 2026-10-17 on a Mac with Apple silicon, macOS 26, inside the machine lock of `tools/serial.sh`. Each mode was held for 2 minutes after 20 seconds of settling, with a reading every 5 seconds.

- Footprint: the highest `phys_footprint` of the process, which is the memory that Activity Monitor shows; graphics memory is included in it.
- Graphics: the highest dirty memory of the category "IOAccelerator (graphics)".
- Rust: the highest count of the bytes that the Rust allocations hold.
- Processor: the share of one core over the 2 minutes, and over the busiest 5 seconds.

| Mode | Footprint | Graphics | Rust | Processor (average) | Processor (busiest 5 s) | Threads |
|---|---|---|---|---|---|---|
| Idle, one server, a text channel shown | 31.8 MiB | 0.5 MiB | 9.8 MiB | 0.3% | 0.4% | 11 |
| Text channel, the list scrolled up 400 messages and back | 34.1 MiB | 0.6 MiB | 11.5 MiB | 26.0% | 31.4% | 12 |
| Voice call with one other person | 42.6 MiB | 0.9 MiB | 14.5 MiB | 8.5% | 9.5% | 17 |
| The same call with the own camera and the camera of the other person | 77.9 MiB | 0.9 MiB | 14.6 MiB | 24.1% | 26.2% | 21 |
| The same call, the own screen shared and the share of the other person watched (1920 × 1080, 30 pictures a second) | 158.6 MiB | 6.2 MiB | 33.6 MiB | 34.6% | 37.7% | 24 |
| A call with three other people, each with a camera (1280 × 720, 30 pictures a second), and the own camera | 90.9 MiB | 1.0 MiB | 16.1 MiB | 25.2% | 26.2% | 22 |
| The call with the two cameras, the own screen also shared (`share-camera`) | not measured yet | | | | | |

These figures are for the code before the changes below. The readings did not grow over the 2 minutes of any mode. After the change of the bar of actions, the idle mode measured 27.2 MiB (Rust 8.5 MiB) and the text mode 30.8 MiB (Rust 10.3 MiB). After the change of the watched share, the share mode measured 125.0 to 138.5 MiB (Rust 15.8 MiB, graphics 1.3 MiB). See "What was changed".

What the figures include and leave out:

- The process also holds the program that drives the window: a runtime with two worker threads, and the frames that it gives the window at 60 a second. In the voice mode its frames took about one tenth of the busy samples of the process.
- The microphone is a generated tone at 44.1 kHz, so the capture is resampled to 48 kHz; most microphones of a Mac give 48 kHz, where the client does not resample. The tone itself is computed on the capture thread.
- The own camera is a generated picture of 640 × 360; a real camera gives 1280 × 720 and needs more encoding.

## Other clients

### Measured here

Slack desktop (Electron), one workspace with one channel shown, nothing done in the window; all 7 processes of the application summed by `tools/process_usage.py` with the same interval and the same readings:

| Client and mode | Footprint | Graphics | Processor (average) | Processor (busiest 5 s) | Threads |
|---|---|---|---|---|---|
| Slack desktop, idle | 584.0 MiB | 3.8 MiB | 0.6% | 1.8% | 126 |
| Junto, idle | 31.8 MiB | 0.5 MiB | 0.3% | 0.4% | 11 |

### Published figures, not measured here

| Client | State | Figure | Source |
|---|---|---|---|
| Slack desktop | One workspace | about 130 MB (10th percentile) to about 960 MB (99th percentile) per workspace | Slack Engineering, "Reducing Slack's memory footprint", 2017-03-03 |
| Discord desktop | Normal use | "less than 1 GB"; the client restarts itself above 4 GB when idle | A Discord employee, quoted by Windows Latest, 2025-12-07 (secondary source) |
| Discord desktop | All states | after the move to Electron 42: median processor time 17% lower, median memory 4% higher; no absolute figure | Discord patch notes, 2026-09-08 |
| Element desktop | Idle with room changes | renderer from about 200 MiB to about 800 MiB, main process sometimes above 500 MiB (version 1.11.20, Linux) | element-web issue 24725, 2023-03-02 (report of a user) |
| Microsoft Teams | 15-minute calls in a virtual desktop | classic client 989 MB, new client 594 MB on average | GigaOm benchmark commissioned by Microsoft, 2025-03-07 |
| TeamSpeak 5 (beta) | Idle | about 900 MB | TeamSpeak community forum, 2020-01-16 (reports of users) |
| TeamSpeak 3, Mumble | Idle | about 50 MB | Reports of users in forums; no measurement with a method was found |

No published figure with a method was found for Discord, Element or Zoom in a voice call, a video call or a screen share.

## What was changed

### The bar of actions of a message is built only under the pointer

Each row of a message carried a bar of five buttons (react, reply, thread, edit, delete) with icons, tooltips and handlers. It was hidden until the pointer was over the row, but gpui builds, lays out and hit-tests a hidden element at each frame; it only does not paint it. While the list scrolled, the window built and laid out one bar for each drawn message at every frame. The shell now keeps the message under the pointer and builds the bar only for it; a scroll of the list removes the bar until the pointer moves again.

| Mode | Footprint before | Footprint after | Rust before | Rust after | Processor per drawing before | after |
|---|---|---|---|---|---|---|
| Idle | 31.8 MiB | 27.2 MiB | 9.8 MiB | 8.5 MiB | (no drawing) | (no drawing) |
| Text, scrolling | 34.1 MiB | 30.8 MiB | 11.5 MiB | 10.3 MiB | 3.17 ms | 2.69 ms |

The processor share of the text mode was 26.0% before and 27.2% after, but the window drew 9,827 times in the 2 minutes before and 12,151 times after: the frame loop of the test is paced by time and a native window waits for the display, so the number of drawings differs between runs. The processor time per drawing is the figure that compares them. Each figure is from one run.

### The watched share is drawn as a camera picture

The engine copied each received picture of the watched share into planes, scaled them to the size of the view, converted them to BGRA and copied the pixels from the buffer of the conversion into a shared slice (`Arc<[u8]>`); the window then copied them again into its image, and gpui copied the image into a texture of its atlas. The engine now keeps the picture as planes of brightness and colour at the size of the view, in the type that camera pictures have (`VideoPicture`, ADR 0774): a received picture at that size is held without a copy, and a larger one is scaled once from its planes where they are (before the scale of the vendored `webrtc-sys`, ADR 0733, it was first copied). On macOS the window draws it from a pixel buffer of Core Video, as it draws a camera, without the conversion to BGRA, without the image and without the atlas. On Windows and Linux gpui has no such path: the planes are converted to BGRA once, into the buffer that the image of the window holds, without a copy. A small picture of a share is a `VideoPicture` as well and is drawn as a camera picture (ADR 0774); before, it kept BGRA pixels in a `Vec`, which the window copied into an image.

Measured on 2026-10-18 and 2026-10-19 on the same Mac by the same method, the share mode only: two runs of the code before, two runs after, and two runs after the method also read the pixel buffers (the category "IOSurface" of `footprint`) and a received picture at the size of the drawing was held without a copy:

| Share mode | Footprint | Graphics | Pixel buffers | Rust | Processor (average) | Processor (busiest 5 s) | Drawings | Processor per drawing |
|---|---|---|---|---|---|---|---|---|
| Before, run 1 | 150.0 MiB | 6.2 MiB | not read | 33.6 MiB | 31.1% | 34.3% | 3,495 | 10.82 ms |
| Before, run 2 | 157.0 MiB | 6.2 MiB | not read | 33.6 MiB | 31.9% | 36.3% | 3,498 | 11.07 ms |
| After, run 1 | 138.5 MiB | 1.3 MiB | not read | 15.7 MiB | 30.4% | 33.1% | 3,534 | 10.46 ms |
| After, run 2 | 125.0 MiB | 1.3 MiB | not read | 15.8 MiB | 32.3% | 33.9% | 3,557 | 11.04 ms |
| After, run 3 | 133.7 MiB | 1.3 MiB | 3.4 MiB | 15.9 MiB | 30.7% | 33.3% | 3,550 | 10.51 ms |
| After, run 4 | 135.5 MiB | 1.3 MiB | 3.4 MiB | 15.9 MiB | 30.6% | 33.1% | 3,544 | 10.49 ms |

The Rust allocations fell by about 18 MiB: the BGRA pictures of the view and their images are gone. The graphics memory fell by about 5 MiB because the atlas holds no texture of the watched share; the pixel buffers that replace those textures are memory of the category "IOSurface", which the graphics column does not include and the footprint does. They held at most 3.4 MiB: two buffers of 1.7 MiB, the one that the window draws and the one converted for its next frame. In this mode the view is smaller than the share of 1920 × 1080, so each picture is scaled and the copy without scaling does not apply; runs 3 and 4 match runs 1 and 2 within the difference between runs. The processor share did not change beyond the difference between two runs: the encoder of the own share takes most of it (see below), and the write into the pixel buffer replaces the conversion to BGRA.

### The processing of the voice works at 48 kHz

The processing module of WebRTC worked at 32 kHz inside, the default of its version: it resampled each frame of the microphone and of the playout from 48 kHz to 32 kHz and back, and removed everything above 16 kHz from the microphone. It now works at 48 kHz (ADR 0733). Measured on 2026-10-23 with `timing::the_processing_of_one_frame_of_each_direction` of the media engine on an x86-64 Linux machine in the test build, with one program for each rate run in turn three times, a frame of the playout and a frame of the microphone with the echo canceller, the noise suppression and the gain control took medians of 202.7, 186.2 and 199.9 µs at 32 kHz and 187.2, 174.7 and 179.4 µs at 48 kHz (each over six rounds of 3,000 frames). At 48 kHz the module has no resamplers, which saves more than its third band costs: about 16 µs for each 10 ms, 0.16 % of one core less.

### The scale of a received picture reads its planes where they are

A received picture in planes that is drawn smaller is scaled from its planes where they are, without a copy of them first (ADR 0774). Measured on 2026-10-23 with `timing::the_scale_of_a_picture_with_and_without_a_copy_of_its_planes` on an x86-64 Linux machine in the test build, in two runs, a picture of 1920 × 1080 scaled to 1128 × 634 took medians of 2.81 and 3.28 ms with the copy and 1.81 and 1.95 ms without it (each over six rounds of 1,000 pictures). A run on a Mac with Apple silicon on 2026-10-22, of which no record was kept, gave 1.36 ms and 1.25 ms; the copy allocates 3 MiB for each picture.

### The emoji of the fonts are measured once per installation and font set

At a start the client measures which emoji of the table the fonts of the system draw (ADR 0758). It lays out 14 probes on the background executor and reads the kept measurement of `emoji-fonts.json`; only when the key of the fonts differs does it lay out the whole table of 3,944 emoji, and on Linux it does so with a text system of its own, which is dropped afterwards. Measured on 2026-10-25 with the target `emoji_fonts` in a release build, the measurement on a thread other than the main thread:

| System | Whole table | Start with the kept measurement | Memory of the process |
|---|---|---|---|
| Linux build image (two processors, DejaVu Sans and Noto Color Emoji 2.042) | 92 ms | 10 ms | resident 15.9 MB before, 18.9 MB after; peak 23.0 MB |
| Windows 11 | not measured in a release build; 0.26 to 0.31 s in a debug build | not measured | not measured |

The kept file held 4,345 bytes of emoji on Linux. A desktop with many more fonts than the build image takes longer for a sign that no font has, because cosmic-text tries every face; that time is on the text system of the measurement, not on the text system of the window. No figure was taken on macOS or on a Linux desktop.

## What uses the memory and the processor

From the profiles of each mode (`JUNTO_TEST_MODES_PROFILE`, with `MallocStackLogging=1`), as allocated bytes of the live blocks:

- Idle and text: most of the footprint is the heap of the process (about 24 MiB of small blocks in the text mode), of which the Rust allocations are 9 to 12 MiB. While the list scrolls, all of the processor time is on the thread of the window, in the frame work of gpui: the layout of the elements (about a quarter of the busy samples), copies of element data, the insertion of the bounds of each element into the tree of bounds, and the building of the rows of the messages.
- Voice: the echo canceller and the other parts of the processing of the voice (WebRTC) on the capture thread, the mixer of the client and a resampler inside the processing on the playout thread, and the Opus encoder and decoder. At the time of these figures the processing of the voice converted the 48 kHz stream to its internal rate of 32 kHz and back; it now works at 48 kHz (see "What was changed"). A block of 7.5 MiB in every call mode is the synthetic sound output of the tests, which keeps 12 seconds of played sound; the product has no such block.
- Camera: the VP8 encoders of the own camera in three simulcast layers (about 16 MiB), the VP8 decoder of the other camera, and the conversion of each received camera picture into a pixel buffer of Core Video on a thread of the runtime of the client.
- Share: the VP8 encoder of the own share holds about 38 MiB and takes about half of the busy samples on three threads, although the share is published as screen content. The decoder of the watched share holds about 17 MiB of frame buffers. Before the change of the watched share, the client held the watched picture converted to BGRA at the size of the view (about 13.5 MiB over 5 pictures), a copy of it as an image of the window (about 9 MiB), and the textures on the GPU (about 11 MiB).

## Open points

- The modes have not been measured on macOS since the own camera, and a share of at most 1280 × 720 at 30 pictures per second, are encoded in H.264 by the hardware encoder of the system (ADR 0737), and the processing of the voice works at 48 kHz (ADR 0733). The table above is from before both changes.
- A Mac converts each received H.264 picture from the pixel buffer of its decoder into new planes (ADR 0774). The cost on the receiving side is not measured.
- Slack was measured only idle. The other modes of Slack and the other clients need a person to start calls and shares in them.

