# 0736. Screen capture: its interface, and the capture of macOS with ScreenCaptureKit

## Status

Accepted

Open: the calls to ScreenCaptureKit were not run. An automated run captures no screen and causes no permission question (ADR 0902), and the capture needs the permission of the application that the system holds responsible. A person runs the first share. The capture of Linux (ADR 0502) and of Windows is not built; on those systems the share button is disabled and says so.

## Context

ADR 0502 decides that the sharer chooses the sound of a share (one application, all system sound, or none), that nothing is chosen for the sharer, that the sound of the client itself is excluded, and that the macOS capture uses ScreenCaptureKit. The step 0 test ran one configuration of a ScreenCaptureKit stream for sound: a filter that selects the sound, 48 kHz stereo, the own process excluded, and a picture of 16 by 16 pixels that is not read. In ScreenCaptureKit one filter decides the picture and the sound of a stream together: a filter of one window carries the sound of the application of that window, and a filter of a screen carries all system sound. The media engine (ADR 0733) takes its devices through an interface so that a test runs the code of the product with a device of the program.

## Decision

Interface:

- The media engine takes the capture through one interface: the state of the permission, the request for it, the opening of the settings of the system, the list of screens, windows and applications, and the start of a capture that gives pictures and interleaved sound to a sink until it is dropped or ends by itself.
- The tests implement the interface with a picture of two known colours and a tone on each channel. The product implements it per system. A system without an implementation answers every request with "not supported".
- A picture crosses the interface as a video buffer of the call library, so the pixel buffer of the system reaches the encoder without a copy.

macOS:

- The binding is `objc2-screen-capture-kit`, which gpui already links. It needs no Swift tool chain at build time.
- A share uses two streams. The stream of the picture has a filter of one screen or of one window, the pixel format `420v`, the size and the frame rate that the person set, and no sound. The stream of the sound has the configuration of the step 0 test, and its filter follows the choice of the sharer: one application (the screen with that application included), all system sound (the screen with nothing excluded), or no stream.
- The two streams are apart, so each choice of sound is possible with each picture, and the sound path is the one that was measured.
- The sound of a shared window, when the person chooses "this application", is the sound of the application that owns the window (ADR 0502).
- A share whose sound stream does not start does not run without sound.
- The system has 10 seconds to answer the request for its lists and the start of each stream. Without an answer the share does not start, and a stream that had started is stopped. A start that completes when nobody waits for it any more stops its stream again.
- The share of a window ends when the system ends its stream, and when the system tells that the stream became inactive, which it does from macOS 15.2 when the window was closed. The stream of the sound ends with the share.
- The size of a picture is the size of the source in pixels, made smaller until it fits the chosen bound (1280 by 720, 1920 by 1080, or no bound), with its proportions and with even sides. A source is not made larger.
- The format of the sound is read from each buffer: 32-bit floats or 16-bit integers, one buffer per channel or one buffer with every channel. Another form gives no sound.

The client itself:

- An application is the client when it has the process identifier of the client, or the same bundle identifier and the same name. No such application and none of its windows is listed. The second rule covers a second instance of the client: the exclusion of the own sound has no effect on a capture that selects an application of the same responsible application (ADR 0502).
- A window is listed when it is an ordinary window on a screen (layer 0), each side has at least 64 points, and it belongs to an application that is not the client.

Permission:

- The client reads the permission with the preflight call of the system, which shows nothing. Without the permission the client asks the system for no list and starts no stream, because each of the two makes the system show its own dialog.
- The dialog of a share then states what the person does in System Settings. Its button makes the system list the application for the permission (the request call of the system) and opens that part of the settings. "Check again" reads the permission again.
- The system does not tell a permission that was never asked for from one that was refused, so the client shows one state for both.

Unsafe code:

- The crate `junto-media` allows unsafe code in a second module: the calls to ScreenCaptureKit, Core Media and Core Graphics. What decides something in that module is a plain function with tests: which application is the client, which windows are listed, and how a sound buffer becomes interleaved samples.

## Consequences

- A share costs two streams of the system. The sound stream captures a picture of 16 by 16 pixels once a second that nobody reads.
- The picture and the sound of a share come from two streams with their own clocks. Each is sent as it arrives; the client does not align them.
- The list of windows and applications is read when the dialog opens. A window that is closed before the person presses "Share" gives "not available any more".
- A person who shares the window of a second instance of the client cannot: it is not listed.
- When the system ends a stream (the window was closed, or the person stopped the capture in the menu bar of the system), the share ends and the person is told. The client does not start it again.
- New direct dependencies of `junto-media` on macOS, all already linked by gpui: `objc2-screen-capture-kit`, `objc2-core-media`, `objc2-core-audio-types`, `objc2-core-foundation` and `objc2-core-graphics`.
