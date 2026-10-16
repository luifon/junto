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
- Captured media is analyzed in memory. Results keep counts, dimensions, timings
  and signal measurements, without saving microphone, camera or desktop recordings.

## Consequences

Native stages require an operator, installed virtual audio and a visible desktop.
They are separate from CI and from generated-media regression tests. Native results
do not establish packaged-app permission identity, acoustic quality, physical
device reconnection or behavior on other operating systems.
