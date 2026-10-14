# 0770. Camera capture and publication

## Status

Accepted

## Context

Voice calls and direct calls need camera pictures without coupling capture to the window. Permission, device disappearance and late completion of capture startup must not leave a camera running after the person stops it or leaves the call.

## Decision

- `CameraCapture` supplies permission state, a device list and a capture stream. Capture starts only after the camera action. The platform implementation may ask for permission then; reading permission or devices does not ask.
- The macOS implementation uses AVFoundation on a capture thread and delivers frames on a serial queue. The preferred capture preset is 1280 by 720; a device that does not support it uses 640 by 480 or 352 by 288, in that order. The selected dimensions accompany the stream. Publication is capped at 30 frames per second and 1.7 Mbit/s, with 360p at 500 kbit/s and 180p at 150 kbit/s as lower layers. The capture preset does not guarantee a particular hardware frame rate.
- The media connection owns capture and publication. A generation identifies each start. Stop and leave release capture; completion of an older start cannot publish a new camera. A connection loss keeps capture while publication is paused, for at most 30 seconds; then the camera ends. A new connection republishes the retained capture at a seat that the session admits, and the connection generation rejects a result from an older room (ADR 0794). A terminal call failure ends the engine and releases capture. Capture shutdown runs on the owning capture thread without blocking the media actor.
- Camera permission and the server's permission to publish a camera are independent of a microphone block. Meeting mode closes microphone publication under ADR 0510; it does not close camera publication.
- Permission refusal, system restriction, a missing device, interrupted capture, unavailable capture, a call that is not connected yet and server prohibition are distinct states. The person starts again after a failure.
- Generated camera sources implement the same interface for tests. They never open a hardware camera or ask for permission.
- The drawing window reports visible camera tiles and their pixel bounds, including interface and display scaling. Remote cameras that are not drawn are disabled; their converted pictures are released. The engine requests a layer for each visible tile and bounds conversion by that tile. The latest frame is retained for drawing. macOS uses one cached native video surface per frame; the fallback converts to BGRA. Only the local preview is mirrored. Published and remote pictures retain their source orientation.

## Consequences

The media engine has no dependency on the window toolkit. The main window or the detached window draws the call; the other releases its image references. Moving the call between windows never opens another capture. Native capture and its permission dialog require separate manual verification; generated-frame tests do not verify hardware behavior.
