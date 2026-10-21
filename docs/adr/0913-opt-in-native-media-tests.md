# 0913. Explicit native media tests

## Status

Accepted

## Context

Generated media verifies call behavior without opening devices. It cannot verify
native audio callbacks, camera capture, or the pixels of a visible window. Native
tests need permissions and must never select a physical sound output by fallback.

## Decision

- A separate example executable, built with `native-device-tests`, runs native
  macOS checks. It requires an explicit runtime opt-in and a named stage. Ordinary
  tests, including ignored tests, never invoke it.
- Permission requests are separate commands, each requiring a matching permission
  opt-in. The operator confirms the responsible application and answers the system
  dialog. Reading states requests no permission.
- Native audio is restricted to one verified BlackHole device. CoreAudio identity,
  device class, virtual transport and driver bundle identity must match, and CPAL
  must expose that same stable identifier in both directions with supported formats.
  Aggregate and multi-output devices are excluded. System defaults are unchanged.
- Before a top-level audio stage opens any stream, CoreAudio must report that
  BlackHole is not running anywhere. Busy, missing and unknown states refuse the
  stage. Its helpers require an admission file in the parent's private temporary
  directory, removed when the stage exits. At the end of the stage, CoreAudio's
  process list must show no process other than the runner and its helpers, which
  run the runner's executable, that uses BlackHole for input or output. A process
  whose device list is unknown, or that uses BlackHole and has an unknown process
  identifier or executable, also fails the stage. CoreAudio lists every process
  that has read one of its properties, and the runner reads them before the
  check, so a list that does not contain the runner, including an empty list,
  fails the stage. D0 checks that this list reports the runner as a client of
  BlackHole while its own output stream runs.
- These checks do not prevent sound from leaving BlackHole. Another application
  can open the device after admission; the end check detects it only if it still
  uses the device when the stage ends, and then the test tones have already
  reached it. The operator must keep monitoring, recording and meeting
  applications stopped throughout the stage.
- A test host checks identity before every open, rejects absent or different
  choices, and verifies the device returned by the production host. Output remains
  zero until that verification completes. Any verification or stream failure
  permanently disarms the host and all of its producers. Recovery uses the same
  checks. Production fallback rules are unchanged.
- Native input opens only after microphone permission is allowed. The audio capture
  and playout measurements run separately so the loopback device cannot feed the
  output of a call back into its input.
- The ordinary camera factory in test-support builds continues to return
  `NoCamera`. The native test feature exposes a separate explicit factory, which
  uses the production implementation and never requests permission during creation.
- A window stage, `w1`, runs a test of the window of `junto-client`, built with its
  `native-device-tests` feature, with the screen capture of the system in the place
  of the synthetic one. The window lists the screens on the executor of gpui, starts
  a share of the first screen without sound through its dialog, and the other side
  of the call receives the pictures. The stage requests no permission and fails
  when the screen permission is missing.
- Captured media is analyzed in memory. Results keep counts, dimensions, timings
  and signal measurements, without saving microphone, camera or desktop recordings.
- The `camera-device` example extends the explicit factory checks to Windows and Linux.
  `--list` opens no capture. Capturing requires `JUNTO_TEST_CAMERA_DEVICE=1`, uses an
  explicitly selected listed device, and checks thirty pictures: each has the dimensions
  of the production stream, a luma plane with more than one value and chroma planes that
  are not empty. It then counts the pictures of three seconds, with the counts of the
  production capture of the device that the feature adds: the pictures that the driver
  gave complete, and, on Linux, the pictures that the driver lost, from the gaps in the
  V4L2 sequence numbers, which a driver skips for a picture that found no queued buffer.
  The driver must lose no picture, so that capture keeps up with the camera, and the
  delivered pictures must be those that the limit of 30 per second keeps of the pictures
  that the driver gave (ADR 0770). A known source must also give at least 80 percent of
  the rate that capture asked the driver for, limited to 30. A real camera may give fewer
  pictures than asked: a USB camera with exposure priority lowers its rate for a longer
  exposure in low light, so for it the rate is printed and does not decide. Media Foundation does not number its samples, so on Windows the
  check of lost pictures is absent. It requests no permission dialog and saves no
  pictures. The optional `rust-linux-devices` job of ADR 0905 authorizes this step
  explicitly; ordinary CI and its ignored tests retain synthetic devices. Linux device
  passthrough exposes only video character devices and their groups. The streaming API
  requires read/write device access even though the test only captures pictures.
- On Windows, `camera-device --virtual` checks the production capture against two virtual
  cameras of Media Foundation (`MFCreateVirtualCamera`, Windows 11) without hardware. The
  example `virtual-camera-source` is a COM server with two sources that generate pictures
  in memory at the rate of their media type: one offers NV12 at 1280 by 720 and YUY2 at
  640 by 480, both at 30 pictures per second; the other offers YUY2 at 1280 by 720 at 10
  and MJPEG at 1280 by 720 at 30, as common USB cameras do, so that capture takes MJPEG and
  decodes it (ADR 0770). Each check registers two new class identifiers as volatile keys of the
  machine, with the profile of each source as a value of its key, because the Frame Server
  runs as a service and reads only the classes of the machine, and keeps the class of an
  identifier that it loaded once, so that a later check with the same identifier would run the
  earlier library; this needs an administrator. It starts each camera for the session of its process,
  captures from it, and checks the declared size of 1280 by 720, the asked rate of 30, the
  luma and chroma of the four quarters of the source within 4 levels, and the measured rate.
  The quarters have strong chroma inside RGB, so that the JPEG pictures, encoded from RGB,
  keep them, and a decoder output left in full range or in another matrix moves Cb and Cr by
  about 8 levels or more. It removes both cameras and both
  keys at its end. Windows does not replace a loaded library file, so each check registers
  a copy of the library named by its process, and removes the copies of earlier checks that
  the Frame Server no longer holds.
- Linux capture is tested without a kernel driver on every commit: `v4l2_fake.c` is a V4L2
  device in memory behind the same table of system calls as the devices of the system. Its
  tests run `v4l2.c` and the Linux capture with programmed formats, frame sizes, intervals,
  colour metadata, buffer counts, damaged and short buffers, a buffer outside the mapped
  ones, refused frame-rate requests, modes that the driver sets otherwise, pictures that
  the driver lost, and refused opens. It keeps the queue of a device for the descriptor that holds its buffers and answers
  busy to another, as V4L2 drivers do, so that a quick stop and start fails without the device
  claim of ADR 0770. Each fake has device identities of its own. Only the tests refer to the
  fake, so the linker leaves it out of the product; the Linux job checks the symbols of the
  client for that (`llvm-nm`). A real device check of Linux capture needs a
  camera or a virtual V4L2 device on the CI host.
- The `rust-linux` job and the job of macOS and Windows lint the media crate with the
  `native-device-tests` feature and all its targets, so that the device checks and the
  native code that only they select are linted on every system.

## Consequences

The macOS native stages require an operator, installed virtual audio and a visible desktop.
The Windows and Linux camera check of a real device requires an explicitly authorized,
accessible camera. The Windows check with virtual cameras requires Windows 11 and an
administrator, and does not test the drivers of real cameras.
They are separate from default CI and from generated-media regression tests. Native results
do not establish packaged-app permission identity, acoustic quality, physical
device reconnection or behavior on other operating systems.
