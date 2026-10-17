# 0733. Media engine: its crate, the sound devices and the audio pipeline

## Status

Accepted

Open: the engine was built and tested on macOS and in a Linux container with virtual sound devices (ADR 0750). The explicit native macOS tier checks the production capture and playout paths through BlackHole (ADR 0913). The Windows build was not run. Physical microphones and physical audio outputs still require operator verification.

## Context

ADR 0701 fixes the LiveKit Rust SDK as the call implementation and names it as the provider of audio devices, echo cancellation, noise suppression and gain control. The client core has every call route and no media (ADR 0720). ADR 0507 requires a level meter per person, a volume per person for each listener and push-to-talk; ADR 0503 requires a sound block that the client applies. ADR 0902 requires tests without a real microphone and without sound from the loudspeakers.

The SDK (`livekit` 0.9.4) offers two ways to carry sound:

- Its platform device module opens the microphone and the output device itself. It plays every remote track through one mix inside the WebRTC library, and the SDK has no call that sets the volume of one remote track. A test cannot give it a device, so the path of the product would not be the path of the tests.
- Its audio source and audio stream take and give PCM frames, and the audio processing module of its WebRTC library (echo cancellation, noise suppression, gain control) can be used on such frames.

## Decision

Crate:

- The crate `junto-media` holds the media engine and the calls of a client (ADR 0734). It depends on `junto-client-core` and on the SDK, and has no dependency on gpui. The desktop client depends on it; the client core and `junto-cli` do not, so they do not link the WebRTC library.
- The SDK is pinned at `livekit` 0.9.4, as the step 0 participant used it. The workspace and the standalone test participant use its build inputs from `vendor/`, with the corrections that this ADR names (publisher negotiation and the order of mute requests, below). Its WebRTC library is downloaded once, when the crate is built first.
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

Executors:

- The window polls the futures of the crate on the executor of gpui, which has no Tokio runtime (ADR 0715). A future that needs a runtime and is polled outside one panics; on the main thread of the window the panic ends the program.
- Any executor can poll a future that a public function of `Calls` returns: `share_sources`, `join`, `join_invited`, `leave`, and the test functions `statistics` and `sound_check`. Such a future waits only for a channel of `tokio::sync` or for a task, which need no runtime to be polled. `Engine::close` also waits only for a channel, and so do the receivers of `tokio::sync::watch` that `Calls` gives for its view and its pictures. The work that needs Tokio (its timers, `tokio::spawn`, its network types and the calls of the call library) runs in tasks of the runtime of the client.
- `Calls::share_sources` calls the source list of the capture in a task of that runtime on every system, and returns a future that waits for the task. Dropping the future ends the task.
- The functions of the device interfaces that return a future (`ScreenCapture::sources` and `start`, `CameraCapture::request_access` and `start`, `MicrophonePermission::request`) are called, and their futures polled, only by the crate and only in tasks of the runtime of the client. Their implementations may use Tokio: the macOS capture bounds its wait for the system with a Tokio timer, and the Linux capture uses D-Bus connections and blocking tasks of the runtime. The desktop client does not call these functions; it uses `Calls`.
- The synthetic devices of `test-support` check in each of their futures that it runs inside a Tokio runtime, and panic otherwise. A window test or a call test then fails where the crate polls the future of a device interface outside the runtime of the client, also when the place would work with the device of one system and not with the device of another.
- Tests: `calls::tests::every_future_of_the_calls_ends_on_an_executor_without_a_tokio_runtime` polls each of these futures with `futures_executor::block_on` on a thread without a runtime, with a capture that waits with a Tokio timer as the macOS capture does. The media live tier does the same in a real call with a share (`executor::the_futures_of_the_calls_end_on_an_executor_without_a_tokio_runtime`). The window test `calls::the_futures_of_the_calls_end_on_the_executor_of_the_window` polls them on the executor of gpui. The native window stage `w1` lists the screens of macOS and starts a share of one through the window (ADR 0913).

Native connection shutdown:

- The native peer-connection wrapper calls `PeerConnectionInterface::Close` synchronously in its destructor, before releasing the interface, observer or factory. The call waits for the signaling thread while the interface still has a positive reference count. Pending callbacks can finish before `Close` unregisters the observer and stops further callbacks. The [pinned implementation](https://github.com/webrtc-sdk/webrtc/blob/89d790b/pc/peer_connection.cc#L1689-L1766) invalidates pending description callbacks and clears the observer before returning.
- This order also applies when runtime shutdown cancels a task, a connection fails during setup, or a close future is dropped. An asynchronous room leave cannot provide that guarantee during runtime shutdown.
- Without this order, the last release enters `PeerConnectionProxyWithInternal` destruction on the runtime thread while the signaling thread can still execute `OnTrack`. That callback copies the wrapper's interface reference to an `RtpTransceiver`; its `AddRef` then calls a pure virtual function on the partially destroyed proxy. The wrapper's observer is also still registered during member destruction.
- The workspace uses the build inputs of `webrtc-sys` 0.3.48 from `vendor/`, with this destructor correction. The WebRTC binary revision is unchanged. The patch is removed when a pinned upstream version guarantees this order. Source provenance and the retained licence notices are kept beside the dependency.
- Native tests observe the closed-state callback on last-owner release, cancellation during runtime shutdown, and a stopped transceiver that outlives its wrapper. Dropping one of several owners does not close the connection; an explicit close followed by destruction remains valid. These checks use no media server or real device.

Publisher negotiation:

- With media server v1.13.9 the SDK connects in its single peer connection mode. A track that another participant publishes while this device is in the room reaches this device only through a new offer of this device: the media server asks for one, and its answer names the track of each media section. The device's own publications use the same offers. A negotiation request that is lost leaves every later remote track and every later publication of the connection without media, while the connection, the devices and the other tracks stay as they were.
- In the fast publish mode the SDK negotiates in one task at a time. A request while the task negotiates makes it negotiate once more after the current answer. In the published version the task set its state to idle and cleared a separate running flag in a later step. A request between those two steps found the state idle and the flag set, marked a retry and started no task; the task then ended. From then on every request found the retry marked and was dropped. In the window live test this left one person unheard for the rest of the call.
- The vendored copy decides with one state under one lock: idle, a task negotiates, or a task negotiates and then negotiates again. A task ends only after it has set the state to idle under that lock, so a request that comes later starts a new task. A request while a task waits for an answer is decided by the same state.
- The decisions are in their own file, which uses the standard library only. `crates/media/tests/negotiation_queue.rs` compiles that file and runs its tests in the workspace suite: a request between the last decision of a task and its end starts a new task, and requests from eight threads leave the queue idle. Both failed with the published decisions.
- This correction is removed when a pinned upstream version decides the queue under one state. Its provenance, the changed files and the retained licence notices are kept beside the dependency.

Order of mute requests:

- A mute or an unmute of a local track sends a request to the media server, which forwards nothing of a muted track. The SDK sends each request from a task of its own, and the task first waits until a restoration of the connection has ended. In the published version every change started a new task. Tasks that waited together for one restoration were released together and sent in the order in which the threads of the runtime ran them. A mute that reached the media server after the unmute that followed it left the track muted at the server while the device sent and showed it open: the others heard nothing for the rest of the connection. The media live test `devices::the_chosen_devices_are_opened_and_a_missing_one_gives_way_to_the_default` failed this way at its last step, when a restoration spanned the close of the setting and the open that followed it.
- The vendored copy sends the mute requests of the local tracks of a participant one after the other. Each request starts on its own task, as before, and sends only after the request that was given before it has ended, or its task has ended without finishing. A request that waits for a restoration therefore holds back the requests after it, and the media server receives them in the order of the changes.
- The ordering is in its own file, which uses the standard library and Tokio only. `crates/media/tests/mute_order.rs` compiles that file and runs its tests in the workspace suite: an unmute does not overtake a mute that waits, 200 requests that each wait a different time arrive in their order on four threads, and a request whose task panics does not hold back the next. The first two failed with a task for each request, as in the published version.
- The media live test `sound::a_mute_and_an_open_during_a_restored_connection_reach_the_media_server_in_order` pauses the media server until the clients restore their connection, mutes and opens the microphone three times 200 ms apart, and checks that the other person hears the device and that the server does not hold its track as muted. It failed in 2 of 8 runs with the published version.
- This correction is removed when a pinned upstream version sends the mute requests of a track in their order. The copy is removed when no correction remains.

Call diagnostics:

- Each media engine has a process-local numeric log identifier. Logs name connection attempts, room connection and closure, sound-state changes, local publication and remote subscription changes and failures. They never log join tokens.
- At the `info` log level, an engine with a room reports sound progress every five seconds: connection age, controls, publication and mute state, remote subscriptions, the capture gate, recent frame counts for each sound stage, dropped capture frames and playback queues. Existing counters are used; no audio samples are logged. The default desktop log level remains `warn`.
- These reports distinguish an unopened microphone, a closed gate, missing publication or subscription, absent incoming frames and playback starvation. They are available in the product as well as in tests, including when a failed wait does not explicitly request a sound report.

TLS trust:

- The pinned WebRTC binary (`webrtc-89d790b`) verifies TLS peers against its compiled-in roots first. Its platform verifier is a fallback: macOS uses an SSL server policy with `SecTrust`; Windows uses the system certificate chain engine and SSL server policy. Linux has no platform verifier in that binary, and its compiled-in roots omit the ISRG roots. A valid TURN TLS certificate can therefore fail on Linux even when the system trusts its issuer.
- The vendored wrapper registers a Linux platform verifier during native runtime initialization, before starting its network threads. Every default TLS adapter then has this fallback, including TURN. `rustls-native-certs` reads the system's certificate files and directories, including the standard `SSL_CERT_FILE` and `SSL_CERT_DIR` overrides. The existing Rustls certificate verifier validates the complete peer chain, its signatures, validity dates, constraints and SSL server purpose against those roots. Roots are loaded for each chain verification so new handshakes see changes to the trust files. An absent usable root or an invalid chain fails verification. The Linux fallback does not fetch revocation information; it uses the installed CA files and the current time.
- On all three systems, a peer is trusted if the compiled-in roots **or** the system trust store accept its chain. The unchanged `SSLPostConnectionCheck` still verifies the hostname. A root distrusted by the system can remain trusted through the compiled-in set. A system-only policy would require rebuilding WebRTC with its built-in roots excluded; this patch does not change that binary or its revision.
- The Linux correction is removed when a pinned upstream version provides system-root verification for every default TLS adapter. The standalone test participant selects the same vendored wrapper as the workspace so deployment checks exercise the correction. Its regression uses the actual peer connection against a loopback TLS server and requires a TURN Allocate request after successful verification. A temporary system-CA file is selected only for the test child process; no host trust store is changed. Unknown issuers, mismatched hostnames and certificates restricted to client use must not receive an Allocate request. Windows still requires its native TLS trust checks in CI.

Extraction of the WebRTC archive:

- The build script of `webrtc-sys` calls `webrtc-sys-build` 0.3.19 to download the WebRTC archive of its target and extract it with `ZipArchive::extract` of the `zip` crate. The published `webrtc-sys-build` 0.3.19 depends on `zip` 0.6.6. Its `extract` keeps the setuid, setgid and sticky bits of an entry (RUSTSEC-2026-0367) and applies the permissions of an entry through an extracted symbolic link without checking that the link target is inside the destination (RUSTSEC-2026-0369). `zip` 9.0.0 corrects both. RUSTSEC-2026-0368 describes a fault of `zip` 9.0.0-pre2 that 9.0.0 also corrects; it names no unaffected earlier version, so `cargo deny` reports it against 0.6.6 as well. No published version of `webrtc-sys-build` depends on a corrected `zip`, and the upstream repository still depends on `zip` 0.6.
- The workspace and the standalone test participant use the build inputs of `webrtc-sys-build` 0.3.19 from `vendor/`. Its manifest depends on `zip` 9.0.4 with the feature `deflate-flate2-zlib-rs` only: the crate reads archives and writes none, and the feature `deflate` of `zip` 9 also adds the zopfli compressor. The source code, the download address and the WebRTC binary revision are unchanged.
- The advisories are not ignored in `deny.toml`. The build reads an archive from the upstream release page over TLS; a changed or replaced archive at that address would otherwise create files with these permissions on the build machine.
- Checks: `cargo deny check` passes, and a build from an empty build directory downloads and extracts the archive with `zip` 9. The extracted tree of `mac-arm64-release` was compared with the one that `zip` 0.6.6 made: the same paths, file types, permissions and contents. `junto-media` builds and links on macOS and in the Linux container, and the media live tier passes.
- This correction is removed when a pinned upstream version of `webrtc-sys-build` depends on a corrected `zip`.

TLS implementation references: [platform selection](https://github.com/webrtc-sdk/webrtc/blob/89d790b/rtc_base/BUILD.gn), [platform verifier registration](https://github.com/webrtc-sdk/webrtc/blob/89d790b/rtc_base/platform_certificate_verifier.h), [verification and hostname checks](https://github.com/webrtc-sdk/webrtc/blob/89d790b/rtc_base/openssl_adapter.cc).

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
- A future of `Calls` that reads from a device costs one task of the runtime of the client. The device interfaces are not an interface for the window: a future of one of them that the window polled would fail on macOS and on Linux.
- A build of the desktop client downloads and links the WebRTC library. The build directory grows by about 1 GiB for it.
