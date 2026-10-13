# junto-media

The media engine carries a client's one call over all its servers. It has no window-toolkit dependency. `Calls` coordinates the server's call membership with the LiveKit Rust SDK; `engine` owns tracks and their processing, and exposes state and the latest pictures for the client to draw.

## Interfaces

`MediaOptions` supplies sound devices, microphone permission, screen capture, camera capture and call timing. Production uses platform implementations. The `test-support` feature supplies generated sound, pictures and camera frames, including delayed startup, permission refusal and device loss.

The audio pipeline uses 48 kHz frames of 10 ms, processing for microphone sound and a mix with volume per participant. Screen-share sound is a separate source and follows the watched share. A server microphone block closes the microphone and share sound; camera publication follows its own source permission.

`CameraCapture` supplies device enumeration, permission state and an owned capture stream. Camera startup is generation-scoped so a late completion cannot restart a stopped camera. Dropping the stream requests capture shutdown without blocking the media actor. Capture survives a media reconnect until the call ends. The macOS implementation uses AVFoundation; other platforms can implement the same interface. Publication uses camera simulcast layers and a 30 fps cap. The local preview is mirrored without mirroring published frames. The drawing window reports visible tiles and their pixel bounds; hidden remote cameras are suspended. Device enumeration belongs to a background refresh, and converted frames share one cached native surface. See ADR 0770.

Screen capture supplies pictures and optional sound through an owned stream. The last still picture is refreshed once a second for late subscribers. Each viewer watches one share and hears only its sound, with an independent volume. Publication uses one full-resolution stream with an explicit resolution-preserving degradation preference. The SDK exposes no minimum bitrate or supported bandwidth-estimate override; low bandwidth may reduce cadence or suspend frames. See ADRs 0736, 0737 and 0773.

On macOS the crate defines `pthread_cond_wait` and `pthread_cond_timedwait` for all statically linked code (`condition_wait`). The WebRTC library ends a blocking call when the system refuses its wait, before the awaited task ran; the functions return such a refusal as a wake-up without a signal, and the wait is repeated. `tools/check-condition-wait.sh` shows the fault and the correction with the linked library and checks the symbols of a built program. See ADR 0786.

## Tests

Run ordinary tests through the shared build lock:

```sh
tools/serial.sh cargo test -p junto-media
```

Live tests require the server binary of the checkout and the repository's PostgreSQL and LiveKit wrappers:

```sh
tools/serial.sh cargo build -p junto-server --bin junto-server
tools/serial.sh tools/with-test-postgres.sh tools/with-test-livekit.sh \
    cargo test -p junto-media -- --ignored
```

Live tests join actual rooms, inspect generated camera frame colors, measure received tones, exercise capture shutdown and reconnect, and verify server blocks and moves. They never open a real microphone, camera or screen capture, play audible sound, or request system permission. A generated-source pass does not verify native capture or its permission dialog.

The desktop crate tests the direct-call ring and window controls with the same injected interfaces. Its offscreen `junto-screens` binary renders camera, direct-call and detached-window states without capturing the display.
