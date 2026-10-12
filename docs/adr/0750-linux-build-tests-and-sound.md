# 0750. Linux build, tests and call sound

## Status

Accepted, with the container verification limits below.

The container checks run on arm64. That is additional build coverage, not evidence for the x86-64 release target of ADR 0705. The x86-64 image build and its toolchain check must be recorded separately from the arm64 workspace and live tests. Hardware devices and hardware graphics drivers require separate tests.

## Context

The native client depends on system libraries for its window, text, sound and call implementation. A build of the Rust workspace alone does not exercise a display server or an audio graph. Sound played through the PulseAudio compatibility service cannot reliably be identified by the socket process identifier of the application (ADR 0502).

## Decision

- `tools/linux-container.sh` delegates to the shared build lock and runs commands in a Linux container. The image uses a pinned Rust base image, pinned system packages, and LLVM package pool filenames with SHA-256 values for arm64 and amd64. Each architecture has an explicit Rust linker. A moving LLVM package index does not select the compiler version.
- Cargo's registry and target directory use named volumes. The retained target volume is for the native architecture and debug profile only. An emulated architecture check uses a temporary target volume that is removed after the check. The source is mounted from the checkout. On a Linux host the build user has the source owner's user and group identifiers. Build scripts therefore have the same trust as a build on the host.
- The runner uses private PostgreSQL and LiveKit containers without published ports. One fixed set of resource names shares the build lock. A leftover container or network stops a new run with an explicit error. The restart test can start and stop only the test LiveKit container through a small control service. The Docker socket path can be supplied for a rootless daemon.
- Each suite repetition uses a fresh runner invocation and a fresh PostgreSQL container, removed with its volume afterwards. Reusing a database container across suites accumulates test databases and statistics. PostgreSQL uses the image selection and database options of `tools/with-test-postgres.sh`, without an extra shared-memory allocation. A failed runner command includes the database’s critical failure diagnostics.
- Test audio uses PipeWire and virtual null devices in a private D-Bus session. No host sound device, display, session bus or home directory is mounted. X11 and Wayland checks use virtual displays. A mapped window is not sufficient: the smoke test also checks that the application painted its surface. The image pins Mesa 25.0.7 from Bookworm backports and uses normal renderer selection; the older Mesa 22.3.6 software Vulkan path painted a blank application under both virtual X servers.
- The Linux sound host uses CPAL's native PipeWire backend. It reports an error if the native library cannot initialise or PipeWire cannot be reached. It does not silently select an ALSA or PulseAudio route, because that would change how the client excludes its own sound from a share.
- The native call library exports PipeWire stubs that also receive the calls made by CPAL. A small native bridge initialises these symbols before either the audio host or screen capture enters PipeWire. This bridge depends on the pinned call library; its declaration is checked when that dependency changes.
- Device listing refreshes a registry snapshot. Opening devices uses that snapshot, and a retained host keeps CPAL's PipeWire initialisation alive between operations. An explicit device identifier that the host cannot open returns an error. The engine retains the default-device selection and retry policy of ADR 0733.
- The Linux build baseline is PipeWire 0.3.65 or newer. PipeWire has supported paired, repeated initialisation and deinitialisation since 0.3.49. Older library versions are outside the supported baseline. The capture library keeps its own initialisation alive while CPAL retains and releases its guards.
- Failure statistics of the live media tests are available only with `test-support`. A measured video interval records counters at both boundaries and the age of the share. A deficient interval prints those records even when a later retry recovers. The measurement does not infer a cause from a single cumulative snapshot.

## Consequences

- A container run proves the tested virtual devices and software renderers. It does not prove microphone permissions, physical hotplug, GPU drivers, a desktop notification daemon or a person's portal chooser.
- The Linux host needs the shared lock script's `flock` path; macOS uses its `lockf` path. The container scripts do not implement a second lock.
- The image and Cargo volumes remain available for subsequent checks. Containers and their private network are removed after each command, including failures. Artifact-copy failures preserve an earlier command failure.
- The Linux sound host and the native bridge are separate from the macOS implementation.

## References

- [PipeWire 0.3.65 initialisation and deinitialisation](https://github.com/PipeWire/pipewire/blob/0.3.65/src/pipewire/pipewire.c).
- [LLVM packages](https://apt.llvm.org/).
