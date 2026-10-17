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

These figures are for the code before the change below. The readings did not grow over the 2 minutes of any mode. After the change, the idle mode measured 27.2 MiB (Rust 8.5 MiB) and the text mode 30.8 MiB (Rust 10.3 MiB); see "What was changed".

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

## What uses the memory and the processor

From the profiles of each mode (`JUNTO_TEST_MODES_PROFILE`, with `MallocStackLogging=1`), as allocated bytes of the live blocks:

- Idle and text: most of the footprint is the heap of the process (about 24 MiB of small blocks in the text mode), of which the Rust allocations are 9 to 12 MiB. While the list scrolls, all of the processor time is on the thread of the window, in the frame work of gpui: the layout of the elements (about a quarter of the busy samples), copies of element data, the insertion of the bounds of each element into the tree of bounds, and the building of the rows of the messages.
- Voice: the echo canceller and the other parts of the processing of the voice (WebRTC) on the capture thread, the mixer of the client and a resampler inside the processing on the playout thread, and the Opus encoder and decoder. The processing of the voice converts the 48 kHz stream to its internal rate and back. A block of 7.5 MiB in every call mode is the synthetic sound output of the tests, which keeps 12 seconds of played sound; the product has no such block.
- Camera: the VP8 encoders of the own camera in three simulcast layers (about 16 MiB), the VP8 decoder of the other camera, and the conversion of each received camera picture into a pixel buffer of Core Video on a thread of the runtime of the client.
- Share: the VP8 encoder of the own share holds about 38 MiB and takes about half of the busy samples on three threads, although the share is published as screen content. The decoder of the watched share holds about 17 MiB of frame buffers. The client holds the watched picture converted to BGRA at the size of the view (about 13.5 MiB over 5 pictures), a copy of it as an image of the window (about 9 MiB), and the textures on the GPU (about 11 MiB).

## Open points

- The own share is encoded by the software VP8 encoder of WebRTC. A hardware encoder of the system (H.264 with VideoToolbox on macOS) would take a large part of the processor time and of the memory of the share mode. It changes the codec of every share, so it needs its own decision and tests on every system and with the recorder.
- `share::picture::convert` writes the BGRA pixels of each share picture into a `Vec` and turns it into an `Arc<[u8]>`, which copies the pixels again; a field of type `Arc<Vec<u8>>` would keep the first buffer. The window then copies the pixels a third time into its image (`render_image`).
- A received share is converted to BGRA and then copied into an image of the window. A camera picture is drawn from a pixel buffer of Core Video without either step. Drawing the watched share in the same way would remove the copy and the conversion to four bytes a pixel.
- The processing of the voice resamples both streams between 48 kHz and its internal rate. Its internal rate is a setting of the WebRTC code in `vendor/`; a change needs a measurement of the processing at 48 kHz.
- Slack was measured only idle. The other modes of Slack and the other clients need a person to start calls and shares in them.

