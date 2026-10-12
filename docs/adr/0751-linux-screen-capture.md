# 0751. Linux screen capture through the desktop portal and PipeWire

## Status

Accepted. Container checks exercise native shared sound and real portal video with the test backend described below. A person’s portal chooser and an unmodified desktop installation remain unverified.

## Context

A Wayland client cannot enumerate another application's windows or obtain their owning process. The desktop portal chooses and authorises a screen or window and supplies a restricted PipeWire connection. It does not provide the application-sound selection required by ADR 0502. A portal's logical size is not necessarily the size of the captured pixels.

## Decision

- Linux implements the same `ScreenCapture` interface as macOS. The source list represents a request to choose a screen or window with the desktop, rather than inventing a display or window identifier. Only source kinds advertised by the portal are offered.
- Source discovery runs on the client’s existing asynchronous runtime, so the window executor can await its D-Bus and PipeWire work without entering that runtime.
- Permission is requested for each new share, after the person presses Share. Listing choices causes no portal dialog. The Linux permission state means that authorisation takes place at the start; it does not claim that the desktop has granted permanent access.
- Each share owns a portal session and a private D-Bus connection. It selects one source, asks for an embedded pointer when the backend supports it, and uses no persistent restore token. Cancellation, refusal, session revocation and a disappearing source end the request or capture. Dropping a pending request closes its resources as well as dropping an active capture.
- Video is read from the remote PipeWire file descriptor supplied by the portal. A stream serial is preferred when the portal supplies it. Legacy node identifiers are used only within the session that supplied them. A removed source is not rebound to another node that reuses its identifier.
- The capture negotiates raw shared-memory pixels, validates buffer boundaries and strides, honours crop metadata, and converts to the call library's video buffer. The negotiated pixel size determines the share dimensions. Resolution and frame rate use the same choices as macOS. An unsupported buffer format fails explicitly.
- The sound choice remains independent and explicit: no sound, system sound, or one application. A Linux window selection does not automatically select its owning application's sound. The person chooses an application from the currently playing applications also when sharing a window. A failed or unavailable sound choice never becomes system sound.
- Audio uses a separate direct connection to the local PipeWire graph, following ADR 0502. It creates explicit links from selected playback streams without automatic session-manager linking. Application identity is the sandbox application identifier where available, otherwise the binary and application names together. A reported process identifier or reusable node identifier does not identify an application.
- The sound graph excludes the capture's client, the daemon-verified process identifier of Junto's playback, and forwarding nodes that have a link group. Missing identity or incomplete node information does not grant permission to capture. Registry changes update both additions and removals of links.
- The audio callback writes stereo pairs to a preallocated queue of at most 200 milliseconds. A separate ten-millisecond clock delivers sound and inserts silence when the graph produces no callbacks. A start or restart primes sixty milliseconds of sound to cover the variable graph quantum. The audio callback does not allocate, take a mutex, wait for the sender or perform file or network work.
- The capture requests nine named input channels: front left and right, centre, subwoofer, rear left and right, side left and right, and mono. The front pair is retained, centre and surround use a factor of one over the square root of two, subwoofer uses one half, and mono goes to both channels. The result is clipped to the sample range. An unsupported channel layout removes the affected application and system-sound choice from the list; a layout that changes to an unsupported one during capture ends the capture.
- The raw video copy is bounded to an 8K frame and a queue of two frames. Row padding and pixels outside crop metadata are not sent. A fully transparent pixel is replaced with black so hidden colour bytes cannot be transmitted. The software conversion happens off the PipeWire event thread.
- A sandbox with access to the portal but no access to the local audio graph can offer video without sound. It must not offer audio choices it cannot provide.
- The virtual portal test builds the pinned wlr 0.7.1 backend with one shared-memory compatibility patch. That backend otherwise rejects the Pixman compositor because it requires a DMA-BUF format in addition to valid shared-memory pixels. The patch retains shared-memory format validation and changes no chooser, permission, transport or product assertion. The source archive is checked by SHA-256 and the patch must match exactly. This backend is confined to the test image.

## Consequences

- The portal owns its chooser and may include Junto's window. The client cannot filter the desktop's window list through the ScreenCast API. Selecting a whole screen can also include Junto, as on macOS. Own application sound remains excluded.
- A backend that supports only whole screens offers no window choice. Automated virtual-display checks and fake portal tests must state which source kinds they exercise.
- A real person's portal permission dialog, desktop-specific window selection and revocation controls require manual validation. A headless test that selects a virtual output is not evidence that those dialogs were run.
- A successful virtual capture with the patched backend does not establish compatibility with an unmodified desktop backend. Both stock wlr 0.7.0 and 0.8.1 rejected the tested Pixman session: the former at format validation and the latter because the old screencopy protocol path requires a DMA-BUF interface.
- A program that bypasses PipeWire has no playback stream to capture. The share does not change its sound selection to compensate.

## References

- [Desktop portal ScreenCast interface](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.ScreenCast.html).
- [Desktop portal request results](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.Request.html).
- [Pinned test backend source](https://github.com/emersion/xdg-desktop-portal-wlr/tree/v0.7.1).
- [PipeWire stream API](https://docs.pipewire.org/group__pw__stream.html).
- ADR 0502 and `spikes/audio-capture-linux/RESULT.md`.
