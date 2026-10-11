# 0701. Native client with gpui and its dependencies

## Status

Accepted, pending step 0

Open: the dependency set and the client functions were not tested on Windows. On Linux the frame rates were not measured on a display that the GPU drives, and a file drop from a file manager was not tested on any system (`docs/step-0/README.md`, test 2).

## Context

The client runs on macOS, Windows and Linux and must provide calls, screen capture and sound capture on each. A webview client uses more memory than a native client, and its call implementation is the one of each system's webview, which differs between systems. The crate named `gpui` on crates.io is far behind the main branch of gpui, and the component library for gpui is tied to a specific series of gpui snapshots. The LiveKit Rust SDK does not capture system sound or application sound.

Step 0 built one test application with one set of crates on macOS and on Linux, in an X11 session and under a Wayland compositor (`docs/step-0/README.md`, test 2).

## Decision

Framework:

- The client is a native application built with gpui, in Rust.
- It runs on macOS, Windows and Linux, on both X11 and Wayland.
- The call implementation is the LiveKit Rust SDK on all three systems.

Dependencies:

- The client uses one fixed set of published crates, all Apache-2.0:
  - gpui: `gpui-pre` 0.3.8 with `gpui-pre-platform` 0.3.8 and the platform crates of the same version;
  - component library: `gpui-component` 0.7.1 with `gpui-base` 0.7.1 and `gpui-kit-assets` 0.7.1.
- The component library requires `gpui-pre` at exactly 0.3.8. The two are updated together.
- The set has no git dependency and no fork of the component library.
- One lock file serves all systems. It is made with the Linux window features `x11` and `wayland` enabled, because a lock file made on macOS without them did not work on Linux.
- The `gpui-pre` crates are snapshots of the gpui sources. They are published by the maintainer of the component library, not by the maintainers of gpui. The platform crate records the source revision of each snapshot.
- The alternative, if the snapshot series stops or falls behind, is to pin the repository of gpui at a revision and the component library at a matching git revision. This alternative was not built.

Libraries:

- The LiveKit Rust SDK provides audio devices, echo cancellation, noise suppression, gain control and screen capture. Screen capture covers a screen or a window, including Wayland through the system portal.
- Capture of system sound and of application sound is the project's own code, with one implementation per operating system (ADR 0502).
- A separate crate provides camera capture.
- The component library provides the text input, the Markdown view and the message list. The Markdown view comes from the unstyled layer `gpui-base`, because only that layer accepts a resolver for image addresses.

Video drawing:

- macOS draws video through the surface path of gpui, from pixel buffers in the NV12 format. With 1, 4 and 9 tiles of 1280×720 at 30 frames per second, every frame was shown, and the process used 7 to 10 % of one core.
- The surface path exists only on macOS. On Windows and Linux each frame of each visible tile is uploaded as an image. Windows was not tested.
- On Linux the client requests a lower resolution for small tiles: 1280×720 only for one tile, and at most 640×360 per tile for four tiles or more. Measured under a headless Wayland compositor that rendered on the GPU, on a machine that other work loaded: one tile of 1280×720 was shown at 30 frames per second, four tiles at 22 to 24, and nine tiles at 11. Nine tiles of 640×360 were shown at 30 frames per second with half of one core.
- When gpui selects a software renderer, the client reduces or stops video and animation.

What the client builds itself, because the set does not provide it:

- The video tile: frames from the call library into pixel buffers on macOS and into images elsewhere, with the release of old images.
- Custom inline elements of a message, such as the mention chip, as plugins of the Markdown view.
- A resolver that loads images from local files and from attachment addresses that need a session.
- An emoji picker on Linux, where gpui opens no system picker.
- The application bundle on macOS. Without a bundle, gpui shows no operating-system notification.
- On Linux, the application identifier of the window and a desktop entry with the same name. Without them the task bar shows a generic icon.

## Consequences

- The client uses less memory than a webview client, and calls behave the same on the three systems. An idle test window used 88 MB of resident memory on macOS and 237 MB on Linux with a proprietary GPU driver, of which 173 MB were mapped driver libraries.
- The client depends on a snapshot series that a third party publishes. An update of gpui requires a snapshot and a release of the component library that match. If the series ends, the client moves to the alternative above.
- The frame rates on Linux were measured without a real display. In the X11 session of the test machine each finished frame was copied to the X server through its socket, so the rates of that session (6 to 11 frames per second) are not rates of a desktop. The tile sizes above are confirmed when the frame rate is measured on a display that the GPU drives.
- A call with many cameras shows tiles at a lower resolution on Linux than on macOS. The limit on Windows is not measured.
- Limits of the fixed set on Linux: a second notification with the same tag does not replace the first, and the call that dismisses a notification does nothing. A selection is not written to the primary selection, and a middle click does not paste; gpui has the calls for it, and the component library does not use them. A file drop from another application failed on X11 (ADR 0703). "Keep on top" needs a request that gpui does not send (ADR 0507).
- A full input method was not tested on any system. Dead keys work on macOS and Linux, and the Compose key works under Wayland.
- Key bindings name the secondary modifier, which is the Command key on macOS and the Control key on Linux.
- A Linux build needs the development files of fontconfig, libxcb, libxkbcommon and libxkbcommon-x11. The packaging facts are in ADR 0705.
- The project maintains three implementations of sound capture.
- Screen reader support depends on gpui (ADR 0702).
- There is no browser client (ADR 0105), so every participant installs the application.
- The licence check allows `CC0-1.0` and `bzip2-1.0.6`, which three packages of the set carry, and ignores the notices that four packages of the set are no longer maintained (`deny.toml`). Each exception ends when the set no longer contains its package.
- The first version of the application uses the unstyled layer `gpui-base` only (ADR 0730); the styled layer and its assets join the lock file with the first screen that needs them.
- Where the Apache-2.0 sources do not cover a need, the reference-code process of ADR 0801 applies. Step 0 met no need that required GPL code.
