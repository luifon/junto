# junto-media

The native client's sound devices, media connection and screen-share capture. The engine has no window dependency. The application supplies an audio host, a microphone-permission interface and a screen-capture interface; tests supply tones, a measured output and synthetic pictures. ADRs 0733–0739 describe the engine and its call behaviour. ADRs 0750–0752 describe the Linux work and its verification limits.

## Linux build and tests

The container runner needs Docker and the shared build lock. Its image uses Rust 1.98.1, pinned system packages and an architecture-specific LLVM installation. It has arm64 and amd64 toolchain definitions. A successful image build is not a workspace test on that architecture.

```sh
tools/linux-container.sh cargo check --workspace --all-targets
tools/linux-container.sh cargo test -p junto-media
tools/linux-container.sh cargo test -p junto-media --no-fail-fast -- --ignored
tools/linux-container.sh cargo clippy --workspace --all-targets -- -D warnings
```

The ignored tier starts real media connections against the runner's LiveKit service. On Linux it also exercises the production sound host with private PipeWire null devices. `--no-fail-fast` lets Cargo reach the separate audio-test binary even when a call test fails. Test concurrency remains the default unless a command explicitly changes it.

The server’s real-media tier also needs its standalone synthetic participant. This helper builds that participant in the same target volume before running the tier:

```sh
tools/linux-container.sh tools/linux/server-live.sh --show-output
```

The runner mounts that target volume at the participant path expected by the server tests. The helper builds the debug participant with the workspace’s debug settings and places a symbolic link at the release path expected by those tests; it does not build a second profile or create a second compilation cache.

Run each full suite repetition through a separate launcher invocation so its database starts empty and is removed with its volume afterwards. Do not run several workspace repetitions inside one container command. PostgreSQL uses the image selection and database options of `tools/with-test-postgres.sh`.

The runner creates its own PostgreSQL and LiveKit services, D-Bus session, PipeWire daemon and virtual sinks. It publishes no service ports and mounts no host display, sound device, session bus or home directory. The build has four Cargo jobs, four CPUs and a 6 GB memory limit. All compilation and test commands share the host build lock.

Call sound uses CPAL's native PipeWire backend. This lets the capture identify Junto's own playback using the process identity that the PipeWire daemon obtains from its socket. A PulseAudio compatibility connection would instead identify the compatibility service. The baseline library is PipeWire 0.3.65; physical devices and older versions need separate validation.

The native call library supplies PipeWire stubs that also intercept calls made by other linked libraries. A small bridge loads those symbols before the audio host uses them. The bridge's declaration depends on the pinned call-library version. Screen capture must use the same initialisation boundary.

The tests do not prove physical microphone or loudspeaker behaviour. Tests that use virtual devices require `JUNTO_TEST_VIRTUAL_AUDIO=1`, which the isolated runner supplies. Do not set it to run them against an ordinary desktop session.

## macOS tests

```sh
tools/with-test-postgres.sh tools/serial.sh cargo test -p junto-media
tools/with-test-postgres.sh tools/with-test-livekit.sh tools/serial.sh \
    cargo test -p junto-media --no-fail-fast -- --ignored
```

The call tests use synthetic capture and playback. They do not open a microphone or start ScreenCaptureKit. The macOS capture's filtering and buffer conversion have unit tests; a real permission request and a real screen or window capture require a manual test.

## Screen capture

The common interface is `share::ScreenCapture`. A `CaptureStream` owns the running capture; dropping it ends the capture. The sink receives pictures, interleaved sound and an end notification. An end notification is terminal. Sound is separate from the microphone pipeline and has no microphone echo cancellation or noise suppression.

macOS uses ScreenCaptureKit. Linux asks the desktop portal to choose and authorise a screen or window, then reads video from its restricted PipeWire connection. The Linux picker lists only the source kinds that the installed portal supports. The desktop owns its chooser; Junto cannot exclude its own window from that chooser. Application sound is an independent, explicit choice from the programs currently playing through the local PipeWire graph. No unavailable or failed application choice becomes system sound. The synthetic implementation remains available through `test-support` on both systems.

Linux shared sound uses explicit playback links, excludes Junto's own process and forwarding nodes, and keeps application identity across stream and process restarts. The direct PipeWire socket must be accessible for those sound choices. A sandbox with portal access but no graph access can offer video with no sound. There is no hardware-device capture path for applications that bypass PipeWire.

The fake portal tests and pixel, identity, routing and clock tests run with the ordinary media unit tests. The ignored audio test uses native and PulseAudio players directed only to a private null sink, and checks selection, own-sound exclusion, mute, pause and restart. A separate test feature enables the real portal test because it needs a particular virtual Wayland session:

```sh
tools/linux-container.sh tools/linux/display.sh wayland tools/linux/portal.sh \
    cargo test -p junto-media --features portal-tests --lib \
    the_real_portal_captures_the_virtual_output_and_drop_stops_delivery -- --ignored --show-output
```

The [portal backend configuration](https://manpages.debian.org/bookworm/xdg-desktop-portal-wlr/xdg-desktop-portal-wlr.5.en.html) selects only `HEADLESS-1` without a chooser. The script refuses a session that does not contain exactly that one virtual output. It must never be used with a real desktop session. This test checks screen pixels and stopping, not a person's permission dialog or a window chooser. The selected backend supports whole screens; fake-service tests cover the window request protocol. The container check verifies two output colours, the selected resolution and no frame delivery after dropping capture, using the test backend described below.

The test image builds a SHA-256-pinned wlr 0.7.1 portal backend with the shared-memory compatibility patch in `tools/linux/install-portal.sh`. Stock wlr 0.7.0 and 0.8.1 rejected the virtual Pixman output because it provides no DMA-BUF format or interface. The patch permits the backend's existing shared-memory path and retains its pixel-format check. The test uses the actual portal protocol and PipeWire transport, but a pass with this patched backend does not replace testing an unmodified desktop installation.

## Diagnostics and retained resources

With `test-support`, a live test can query connection statistics. A deficient frame-rate interval prints snapshots from its boundaries, the selected share and the share's age. Add `--show-output` to see diagnostics from a deficient interval that recovered before the test ended. Successful intervals do not print statistics.

The image `junto-linux1-build:1` and volumes `junto-linux1-cargo` and `junto-linux1-target` remain for later commands. Keep the target volume to one architecture and the debug profile. An emulated architecture check needs a temporary target volume, removed when the check finishes. The runner removes its containers and network after each command. It refuses an existing resource name rather than replacing another run. On Linux it builds with the invoking user's user and group identifiers. `JUNTO_DOCKER_SOCKET` supplies a nonstandard local Docker socket, such as a rootless daemon's socket.

`JUNTO_LINUX_ARTIFACTS` names a host directory to receive `/tmp/junto-linux1-artifacts` from the build container. The command must create that container directory when it produces artifacts. A copy failure is reported and does not replace an earlier command failure with a different exit status.
