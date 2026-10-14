# 0733. Media engine: its crate, the sound devices and the audio pipeline

## Status

Accepted

Open: the engine was built and tested on macOS and in a Linux container with virtual sound devices (ADR 0750). The Windows build was not run. No real microphone and no real output device was opened by a test (ADR 0902); a person does the first call with real devices.

## Context

ADR 0701 fixes the LiveKit Rust SDK as the call implementation and names it as the provider of audio devices, echo cancellation, noise suppression and gain control. The client core has every call route and no media (ADR 0720). ADR 0507 requires a level meter per person, a volume per person for each listener and push-to-talk; ADR 0503 requires a sound block that the client applies. ADR 0902 requires tests without a real microphone and without sound from the loudspeakers.

The SDK (`livekit` 0.9.4) offers two ways to carry sound:

- Its platform device module opens the microphone and the output device itself. It plays every remote track through one mix inside the WebRTC library, and the SDK has no call that sets the volume of one remote track. A test cannot give it a device, so the path of the product would not be the path of the tests.
- Its audio source and audio stream take and give PCM frames, and the audio processing module of its WebRTC library (echo cancellation, noise suppression, gain control) can be used on such frames.

## Decision

Crate:

- The crate `junto-media` holds the media engine and the calls of a client (ADR 0734). It depends on `junto-client-core` and on the SDK, and has no dependency on gpui. The desktop client depends on it; the client core and `junto-cli` do not, so they do not link the WebRTC library.
- The SDK is pinned at `livekit` 0.9.4, as the step 0 participant used it. Its WebRTC library is downloaded once, when the crate is built first.
- A binary that links the crate passes `-ObjC` to the linker on macOS, in its build script. Without it the binary stops at its start.

Sound devices:

- The engine opens sound devices through one interface: list the devices, open an input, open an output. The product implements it with `cpal` 0.18 (CoreAudio, WASAPI, PipeWire). The tests implement it with a tone as the microphone, a sink that keeps what it was given, and a device list that a test changes.
- The device module of the SDK is not used. This changes ADR 0701: the SDK provides the processing, and `cpal` provides the devices.
- On Linux the PipeWire host of `cpal` is used, so the sound of a call is played by the client's own PipeWire connection (ADR 0502). The device list and subsequent stream opens use one host snapshot. A failed PipeWire initialisation or connection is reported, without falling back to ALSA or PulseAudio (ADR 0750).
- A chosen device is used while it is connected. When it is not, the default of the system is opened, and the choice stays stored. A microphone that is opened in this way during a call, in the place of the microphone that the call used before (after a failure, at a new media connection, or after a block), sends nothing until the person opens it (ADR 0794). The chosen device that is connected again is used without that.
- A stream that fails is opened again once, on the device that the rule gives then. When the stream that was opened in this way fails within 5 seconds, the device stays closed and its failure is shown. A microphone is opened again when the person asks for it or chooses a device; an output when the person chooses a device or changes a sound setting. A microphone that could not be opened is not tried again by itself either.
- A device is opened with its default format when the pipeline converts the rate of that format. Otherwise it is opened at another rate that it offers, 48 kHz first, with the channels and the sample type of the default where it offers them. A device without such a rate is not opened.
- The engine reads the microphone permission of the system before it opens a microphone. `not asked`: it makes the system ask, and opens the microphone after a positive answer. `refused`: it opens no microphone. The permission is behind an interface; the product reads it through AVFoundation on macOS, and a system without such a permission counts as allowed.

Pipeline:

- Every stage works at 48 kHz in frames of 10 ms, which the processing module and the audio source of the SDK take.
- The conversion between a device and the pipeline works in steps that are a whole number of samples on both sides, so the time of the device and the time of the pipeline stay equal. A step is 10 ms at a rate that 100 divides, such as 44.1 kHz and 48 kHz, 20 ms at 22.05 kHz and 40 ms at 11.025 kHz. The rates from 8 kHz to 192 kHz with a step of at most 40 ms are converted. The resampler of the SDK takes 10 ms at a time; a longer step is given to it as 10 ms at rates that are as many times higher, which is the same ratio.
- Microphone: the samples of the device become mono (the mean of its first two channels) and are resampled; the processing module processes each frame; the level is measured; the frame is sent, or replaced by silence while the microphone is muted or push-to-talk waits for its key; the audio source of the published track takes the frame. The audio source applies no processing of its own.
- Up to 500 ms of frames wait when the media connection takes them late. The mute and the key are read when a frame is made and again when the connection takes it. A frame that waited is sent as silence when the microphone is muted or the key is released at that time, and also when either happened at any time since the frame was made.
- Playout: each remote audio track is read as stereo frames into a queue of its own. The output device asks for samples; the mixer takes 10 ms of each queue, multiplies by the volume of the person (0 to 200 %), adds, and limits the sum. The mix is given to the processing module as the reference of the echo canceller and is converted to the format of the device.
- A queue starts to play when it holds 30 ms, and again after it ran empty. It never holds more than 200 ms; older sound is dropped.
- Sound off, by the person or by a sound block (ADR 0503), plays silence and still takes the tracks, so that nothing old is played afterwards.
- The level of a source rises at once and falls by 0.6 dB per frame. A meter is empty at -60 dBFS and full at -12 dBFS. A person speaks while the level of what the microphone sends is above -45 dBFS, and for 400 ms after it.
- A person can turn each part of the processing off. With all three off, the frames pass unchanged. While at least one part is on, the high-pass filter of the module is on too: it removes hum below speech, and the person has no setting for it.

Engine:

- One engine is one media connection. One task owns the room, the microphone track, the remote tracks and the two devices; a caller sends it commands and reads two values that it keeps current: the state and the levels.
- The microphone is open while the device is in the room, no microphone block is told by the server (ADR 0734), and the permission of the participant in the room allows a microphone track. The server puts a microphone block into the join token and into that permission (ADR 0510), so the block holds from the first moment of a connection: a blocked person who enters again opens no microphone. The microphone track is published while the microphone is open. A block withdraws the track and closes the microphone; a mute keeps both and sends silence, and is told to the room as a muted track.
- A publication that the media server refuses is tried again every second, while the microphone may be sent.
- When the call library connects anew, it withdraws the microphone track and publishes it again under another identifier. The engine waits for that track for 10 seconds and keeps the audio source that the track is bound to. The track that returns with the identifier of the withdrawn one is the microphone track again; another one is withdrawn. A track that does not return is published anew while the microphone may be sent.
- When the engine leaves a room, it closes both devices first and then waits at most 3 seconds for the media server to confirm the leave.
- The call library restores an interrupted connection by itself. When it gives up, or the media server ends the connection, the engine closes both devices and waits for a new token (ADR 0734).

## Consequences

- The client has a volume per person and a level per person, and the tests carry a tone through the same code as a voice of the product: only the device differs.
- The project owns the pipeline between the devices and the SDK: the conversion, the mix, the queues and the use of the processing module. An error there is an error of the project.
- The echo canceller gets its reference from the mix that the client plays. It estimates the delay between the mix and the microphone by itself; no delay is given to it.
- New dependencies: `cpal` (Apache-2.0) for the devices, and on macOS `objc2-av-foundation`, `objc2-foundation`, `objc2` and `block2` for the microphone permission. `cargo deny check` passes with them.
- The platform bindings contain the unsafe calls: AVFoundation and ScreenCaptureKit on macOS, and the PipeWire initialisation binding on Linux. The selection of a Linux host and its device snapshot remain safe Rust (ADRs 0736 and 0750).
- A muted microphone stays open, so the system shows its microphone indicator while a person is muted in a call. Unmute has no delay.
- The output follows a change of the default device of the system only as far as `cpal` does. A person chooses a device in the call when the sound does not follow.
- At 22.05 kHz and 11.025 kHz the frames of a microphone are made two and four at a time, which adds up to 10 ms and 30 ms of delay.
- A device with more than two input channels gives its first two as the microphone. A device with more than two output channels plays the mix on its first two.
- A build of the desktop client downloads and links the WebRTC library. The build directory grows by about 1 GiB for it.
