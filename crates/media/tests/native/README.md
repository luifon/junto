# Native macOS media checks

These stages open native devices. They require an operator-supervised run, a
BlackHole 2-channel installation, and an exclusive screen interval for fixtures.
They never change the system's default audio route. No physical microphone or
physical sound output is used. Camera and screen media stay in memory.

Build and run through `tools/native-devices.sh`. Set
`JUNTO_TEST_NATIVE_DEVICES=1`; without it the runner fails with `NOT RUN`.
The executable is an example behind `native-device-tests`, so ordinary tests and
`cargo test -- --ignored` cannot open its devices.

Before a stage using local services, set all six isolated test variables:
`JUNTO_TEST_POSTGRES_CONTAINER`, `JUNTO_TEST_LIVEKIT_CONTAINER`,
`JUNTO_TEST_WEBHOOK_ADDRESS`, `JUNTO_TEST_LIVEKIT_PORT`,
`JUNTO_TEST_LIVEKIT_TCP_PORT`, and `JUNTO_TEST_LIVEKIT_UDP_PORT`.
The runner takes the machine lock, builds the needed executables, starts temporary
services, and removes their containers after the stage. Screen stages also build
an AppKit fixture and launch temporary application bundles with distinct identities.
The fixture apps exit when their control connection closes.
A fixture window opens on the first screen, the one with the menu bar, unless that
display's colour space clips one of the four fixture colours; then it uses the next
screen that shows all four. The colour check uses a 12-level tolerance, so a
clipping display would fail it for a reason outside the capture code. With no such
screen, the fixture refuses to start.

| Stage | Evidence |
| --- | --- |
| `inspect` | Read-only native permission state and exact CoreAudio/CPAL BlackHole identity. |
| `d0` | Zero-filled native output, native input callbacks, format and missing-choice refusal. |
| `a1` | Native capture through the call engine and LiveKit, fresh tones and silence, mute and push-to-talk. |
| `a2` | Native playout measured by a BlackHole input probe, volume, sound-off and leave, with microphone publication disabled. |
| `a3` | Native stop/start and refusal of the engine's default fallback for a missing choice. |
| `c1` | Native camera dimensions, conversion to the 601 matrix (every frame that is not a native 601 buffer arrives converted), publication, remote progression, stop and reopen. |
| `s1-s2` | Real display/window marker pixels locally and remotely, source closure and capture-sink release. |
| `s3` | Distinct application tones, own-process exclusion, system sound and sound None. |

Permission requests are separate operator steps. Ask before each possible prompt.
For `request-microphone`, `request-camera` or `request-screen`, set
`JUNTO_TEST_PERMISSION_REQUEST` to the corresponding permission name as well.
Record the app named by the prompt. Camera and microphone need the embedded usage
descriptions; screen access belongs to the responsible application. The operator
handles System Settings and any requested application restart. Resume by reading
states again, without repeating requests automatically.

A camera stage uses the sole listed camera explicitly; if several are available,
set `JUNTO_TEST_CAMERA_ID` to a listed ID. The camera must already be authorized.

Audio uses one second of queue drain and five seconds of measured callback frames,
within ten seconds. Expected tones must exceed -30 dBFS; absent tones must be below
-55 dBFS. Raw capture must match the generated level within 2 dB. Half volume must
reduce the level by 6.0206 dB within 2 dB. Nonfinite and clipped PCM fail. There are
no retries until a measurement passes.

The audio guard fails permanently on a rejected choice, identity change or stream
error. It never delegates a default request, and output remains zero until the
returned stream's device is verified. All fixture players use this same guard.
Do not run a monitoring application that copies BlackHole input to physical output.

The assisted camera card/orientation check and native client controls, preview and
detach/reattach require the operator; the generated window tests do not replace
these checks. Screen sink release proves removal of callback owners; the public
capture interface does not expose ScreenCaptureKit's asynchronous stop completion.
Packaged application permissions, physical device loss, acoustic quality, multiple
display arrangements and other operating systems need separate runs.

Audio stages refuse admission unless CoreAudio reports BlackHole idle before the
first stream opens. Fixture audio requires the admitted parent's private file.
At the end of a stage, a process other than the runner and its helpers that
still uses BlackHole fails the stage, and so does a CoreAudio process list that
does not contain the runner. These checks do not prevent another
application from opening BlackHole during the stage and receiving the tones;
keep monitoring, recording and meeting applications stopped for the entire stage.
