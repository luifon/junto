# junto-media

The media engine carries a client's one call over all its servers. It has no window-toolkit dependency. `Calls` coordinates the server's call membership with the LiveKit Rust SDK; `engine` owns tracks and their processing, and exposes state and the latest pictures for the client to draw. ADRs 0733–0739 describe the engine and its call behaviour. ADRs 0750–0752 describe the Linux work and its verification limits.

## Interfaces

`MediaOptions` supplies sound devices, microphone permission, screen capture, camera capture and call timing. Production uses platform implementations. The `test-support` feature supplies generated sound, pictures and camera frames, including delayed startup, permission refusal and device loss.

The audio pipeline uses 48 kHz frames of 10 ms, processing for microphone sound and a mix with volume per participant. Screen-share sound is a separate source and follows the watched share. A server microphone block closes the microphone and share sound; camera publication follows its own source permission.

`CameraCapture` supplies device enumeration, permission state and an owned capture stream. Camera startup is generation-scoped so a late completion cannot restart a stopped camera. Dropping the stream requests capture shutdown without blocking the media actor. Capture survives a media reconnect until the call ends. The macOS implementation uses AVFoundation; other platforms can implement the same interface. Publication uses camera simulcast layers and a 30 fps cap. The local preview is mirrored without mirroring published frames. The drawing window reports visible tiles and their pixel bounds; hidden remote cameras are suspended. Device enumeration belongs to a background refresh, and converted frames share one cached native surface. See ADR 0770.

Screen capture supplies pictures and optional sound through an owned stream. The last still picture is refreshed once a second for late subscribers. Each viewer watches one share and hears only its sound, with an independent volume. Publication uses one full-resolution stream with an explicit resolution-preserving degradation preference. The SDK exposes no minimum bitrate or supported bandwidth-estimate override; low bandwidth may reduce cadence or suspend frames. See ADRs 0736, 0737 and 0773.

On macOS the crate defines `pthread_cond_wait` and `pthread_cond_timedwait` for all statically linked code (`condition_wait`). The WebRTC library ends a blocking call when the system refuses its wait, before the awaited task ran; the functions return such a refusal as a wake-up without a signal, and the wait is repeated. `tools/check-condition-wait.sh` shows the fault and the correction with the linked library and checks the symbols of a built program. See ADR 0786.

## Tests on macOS

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

The call tests use synthetic capture and playback. They do not open a microphone or start ScreenCaptureKit. The macOS capture's filtering and buffer conversion have unit tests; a real permission request and a real screen or window capture require a manual test.

## Linux build and tests

The container runner needs Docker and the shared build lock. Its image uses Rust 1.98.1 and system packages with exact versions, all from one dated snapshot of the Debian archives, which keeps every version. The call library needs clang 21 or later; the image installs Debian's clang 22 for this release, one version for arm64 and amd64. It has arm64 and amd64 toolchain definitions. A successful image build is not a workspace test on that architecture.

```sh
tools/linux-container.sh cargo check --workspace --all-targets
tools/linux-container.sh cargo test -p junto-media
tools/linux-container.sh cargo test -p junto-media --no-fail-fast -- --ignored
tools/linux-container.sh cargo clippy --workspace --all-targets -- -D warnings
```

The ignored tier starts real media connections against the runner's LiveKit service. On Linux it also exercises the production sound host with private PipeWire null devices. `--no-fail-fast` lets Cargo reach the separate audio-test binary even when a call test fails. Test concurrency remains the default unless a command explicitly changes it.

The Linux ignored tier also runs `tls_trust`: the actual call library sends a TURN Allocate request to a loopback TLS server with a locally trusted certificate chain. It rejects an unknown issuer, a wrong hostname and a certificate restricted to client use. Python and OpenSSL generate fresh keys and certificate chains in a temporary directory; `SSL_CERT_FILE` selects the test root only in a child test process. No host trust store or real network service is changed. To run this check alone:

```sh
tools/linux-container.sh cargo test -p junto-media --test tls_trust -- --ignored
```

The server’s real-media tier also needs its standalone synthetic participant. This helper builds that participant in the same target volume before running the tier:

```sh
tools/linux-container.sh tools/linux/server-live.sh --show-output
```

The runner mounts that target volume at the participant path expected by the server tests. The helper builds the debug participant with the workspace’s debug settings and places a symbolic link at the release path expected by those tests; it does not build a second profile or create a second compilation cache.

Run each full suite repetition through a separate launcher invocation so its database starts empty and is removed with its volume afterwards. Do not run several workspace repetitions inside one container command. PostgreSQL uses the image selection and database options of `tools/with-test-postgres.sh`.

The runner creates its own PostgreSQL and LiveKit services, D-Bus session, PipeWire daemon and virtual sinks. It publishes no service ports and mounts no host display, sound device, session bus or home directory. The build has four Cargo jobs, at most four processors and a 6 GB memory limit. The processors are given as a set and not as a quota: a quota stops every thread of the container for the rest of each 100 ms in which it is used up, and the live tests, which measure sound in real time, then measure those stops. All compilation and test commands share the host build lock.

Call sound uses CPAL's native PipeWire backend. This lets the capture identify Junto's own playback using the process identity that the PipeWire daemon obtains from its socket. A PulseAudio compatibility connection would instead identify the compatibility service. The baseline library is PipeWire 0.3.65; physical devices and older versions need separate validation.

The native call library supplies PipeWire stubs that also intercept calls made by other linked libraries. A small bridge loads those symbols before the audio host uses them. The bridge's declaration depends on the pinned call-library version. Screen capture must use the same initialisation boundary.

The tests do not prove physical microphone or loudspeaker behaviour. Tests that use virtual devices require `JUNTO_TEST_VIRTUAL_AUDIO=1`, which the isolated runner supplies. Do not set it to run them against an ordinary desktop session.

### Address check of the sound capture

The sound capture holds PipeWire objects through a safe Rust interface, and a wrong order of release writes into freed memory without a failing test: the standard allocator hides it. This check runs the Linux sound-capture tests with the address checking of the compiler (`-Zsanitizer=address`), which reports such a write and ends the test:

```sh
tools/linux-container.sh tools/linux/display.sh wayland tools/linux/portal.sh \
    tools/linux/address-check.sh
```

The script installs the nightly toolchain `nightly-2026-10-01` into a temporary directory of the container, builds `junto-media` for the host target with the checking into a directory of the container, and runs the ignored tests of `share::linux::audio::tests` and, under the virtual portal of this command, the picture capture test `share::linux::live`. Without the display and portal scripts it runs the sound tests only. The toolchain and the build end with the container; the build volume is not used. Run it after a change of `src/share/linux/registry.rs`, `connection.rs`, `audio.rs` or `video.rs`, and after a new version of the `pipewire` crate. A run takes a full build of the dependencies of `junto-media`.

## Screen capture

The common interface is `share::ScreenCapture`. A `CaptureStream` owns the running capture; dropping it ends the capture. The sink receives pictures, interleaved sound and an end notification. An end notification is terminal. Sound is separate from the microphone pipeline and has no microphone echo cancellation or noise suppression.

macOS uses ScreenCaptureKit. Linux asks the desktop portal to choose and authorise a screen or window, then reads video from its restricted PipeWire connection. The Linux picker lists only the source kinds that the installed portal supports. The desktop owns its chooser; Junto cannot exclude its own window from that chooser. Application sound is an independent, explicit choice from the programs currently playing through the local PipeWire graph. No unavailable or failed application choice becomes system sound. The synthetic implementation remains available through `test-support` on both systems.

Linux shared sound uses explicit playback links, excludes Junto's own process and forwarding nodes, and keeps application identity across stream and process restarts. The direct PipeWire socket must be accessible for those sound choices. A sandbox with portal access but no graph access can offer video with no sound. There is no hardware-device capture path for applications that bypass PipeWire.

The fake portal tests and pixel, identity, routing and clock tests run with the ordinary media unit tests. The ignored audio tests use native and PulseAudio players directed only to a private null sink. One checks selection, own-sound exclusion, mute, pause and restart; it first measures Junto's own tone at the monitor of its output and counts the frames that its output played, so that it cannot pass while the own tone does not play. One ends a player during a capture, one starts and ends players for 30 seconds during a capture, and one starts a capture while nothing plays and then hears a player. A separate test feature enables the real portal test because it needs a particular virtual Wayland session:

```sh
tools/linux-container.sh tools/linux/display.sh wayland tools/linux/portal.sh \
    cargo test -p junto-media --features portal-tests --lib \
    the_real_portal_captures_the_virtual_output_and_drop_stops_delivery -- --ignored --show-output
```

The [portal backend configuration](https://manpages.debian.org/bookworm/xdg-desktop-portal-wlr/xdg-desktop-portal-wlr.5.en.html) selects only `HEADLESS-1` without a chooser. The script refuses a session that does not contain exactly that one virtual output. It must never be used with a real desktop session. This test checks screen pixels and stopping, not a person's permission dialog or a window chooser. The selected backend supports whole screens; fake-service tests cover the window request protocol. The container check verifies two output colours, the selected resolution and no frame delivery after dropping capture, using the test backend described below.

The test image builds a SHA-256-pinned wlr 0.7.1 portal backend with the shared-memory compatibility patch in `tools/linux/install-portal.sh`. Stock wlr 0.7.0 and 0.8.1 rejected the virtual Pixman output because it provides no DMA-BUF format or interface. The patch permits the backend's existing shared-memory path and retains its pixel-format check. The test uses the actual portal protocol and PipeWire transport, but a pass with this patched backend does not replace testing an unmodified desktop installation.

## Diagnostics and retained resources

With `JUNTO_LOG_LEVEL=info`, each connected engine logs sound progress every five seconds, labeled by engine and connection attempt. The log includes state and track changes and subscription failures, without tokens or audio samples. Both the media and window live harnesses capture these logs at `info` by default, and respect `JUNTO_LOG_LEVEL`. With `test-support`, a live test can also query connection statistics, and `Calls::sound_check` gives what the engine knows of its sound: its states, the remote audio tracks, and for each 10 ms of the last 600 ms the number of frames that were captured, sent, received and played. The window test of a call prints it for both sides when a tone does not arrive. A deficient frame-rate interval prints snapshots from its boundaries, the selected share and the share's age. Add `--show-output` to see diagnostics from a deficient interval that recovered before the test ended. Successful intervals do not print statistics.

The image `junto-linux-build:1` and volumes `junto-linux-cargo` and `junto-linux-target` remain for later commands. `JUNTO_LINUX_PREFIX` replaces the prefix `junto-linux` in the names of the image, the volumes, the containers and the network, for two checkouts on one machine. Keep the target volume to one architecture and the debug profile. An emulated architecture check needs a temporary target volume, removed when the check finishes. The runner removes its containers and network after each command. It refuses an existing resource name rather than replacing another run. On Linux it builds with the invoking user's user and group identifiers. `JUNTO_DOCKER_SOCKET` supplies a nonstandard local Docker socket, such as a rootless daemon's socket.

`JUNTO_LINUX_ARTIFACTS` names a host directory to receive `/tmp/junto-linux-artifacts` from the build container. The command must create that container directory when it produces artifacts. A copy failure is reported and does not replace an earlier command failure with a different exit status.

The workspace patches the native peer-connection destructor to close before releasing its interface and observer, including when runtime shutdown cancels a task. The build inputs and provenance are in `vendor/`; ADR 0733 records the order and the native regression checks.

The workspace also patches the publisher negotiation queue of the call library, so that a request that comes while a negotiation task ends starts a new negotiation. Without it, one lost request left every later remote track of the connection without media. `tests/negotiation_queue.rs` runs the queue tests; ADR 0733 records the defect, and `vendor/README.md` the provenance.

It also sends the mute and unmute requests of a track to the media server in the order of the changes. Without it, a mute and an unmute that waited for one restored connection could arrive in the other order, and the media server held an open microphone as muted. `tests/mute_order.rs` runs the ordering tests; ADR 0733 records the defect.
