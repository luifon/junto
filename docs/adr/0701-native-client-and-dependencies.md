# 0701. Native client with gpui and its dependencies

## Status

Accepted, pending step 0

## Context

The client runs on macOS, Windows and Linux and must provide calls, screen capture and sound capture on each. A webview client uses more memory than a native client, and its call implementation is the one of each system's webview, which differs between systems. The published gpui crate is far behind the main branch of gpui, and the component library for gpui is tied to a specific series of gpui snapshots. The LiveKit Rust SDK does not capture system sound or application sound.

## Decision

Framework:

- The client is a native application built with gpui, in Rust.
- It runs on macOS, Windows and Linux, on both X11 and Wayland.
- The call implementation is the LiveKit Rust SDK on all three systems.

Dependencies:

- Step 0 fixes one set of revisions of gpui and of the component library that work together, or decides to fork the component library (`docs/step-0/README.md`, test 2).

Libraries:

- The LiveKit Rust SDK provides audio devices, echo cancellation, noise suppression, gain control and screen capture. Screen capture covers a screen or a window, including Wayland through the system portal.
- Capture of system sound and of application sound is the project's own code, with one implementation per operating system (ADR 0502).
- A separate crate provides camera capture.
- Apache-2.0 component libraries for gpui are used where they fit: text input, Markdown view, message list.

Video drawing:

- macOS has an efficient path for drawing video.
- On Windows and Linux each frame of each visible tile is uploaded as an image, so the number of video tiles shown at once is limited on those systems.

## Consequences

- The client uses less memory than a webview client, and calls behave the same on the three systems.
- The client depends on revisions of gpui that are not published as a crate release. An update of gpui requires a matching revision of the component library, or work on the fork.
- The project maintains three implementations of sound capture.
- A call with many cameras shows fewer video tiles at once on Windows and Linux than on macOS.
- Screen reader support depends on gpui (ADR 0702).
- There is no browser client (ADR 0105), so every participant installs the application.
- Where the Apache-2.0 sources do not cover a need, the reference-code process of ADR 0801 applies.
