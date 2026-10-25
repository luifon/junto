# 0736. Screen capture: its interface, and the capture of macOS with ScreenCaptureKit

## Status

Accepted

The explicit native macOS tier verifies display and window pixels and application/system audio through ScreenCaptureKit (ADR 0913). Ordinary CI tests capture no screen and cause no permission question.

Open: Packaged-app permission identity and system stop-completion remain separate verification items. ADR 0751 records the Linux implementation and ADR 0706 the Windows implementation, each with its verification status.

## Context

ADR 0502 decides that the sharer chooses the sound of a share (one application, all system sound, or none), that nothing is chosen for the sharer, that the sound of the client itself is excluded, and that the macOS capture uses ScreenCaptureKit. The step 0 test ran one configuration of a ScreenCaptureKit stream for sound: a filter that selects the sound, 48 kHz stereo, the own process excluded, and a picture of 16 by 16 pixels that is not read. In ScreenCaptureKit one filter decides the picture and the sound of a stream together: a filter of one window carries the sound of the application of that window, and a filter of a screen carries all system sound. The media engine (ADR 0733) takes its devices through an interface so that a test runs the code of the product with a device of the program.

## Decision

Interface:

- The media engine takes the capture through one interface: the state of the permission, the request for it, the opening of the settings of the system, the list of screens, windows and applications, and the start of a capture that gives pictures and interleaved sound to a sink until it is dropped or ends by itself.
- The tests implement the interface with a picture of two known colours and a tone on each channel. The product implements it per system. A system without an implementation answers every request with "not supported".
- The interface also gives one still picture of a listed screen or window for the dialog of a share (`ScreenCapture::preview`), at most 320 by 180 pixels (`PREVIEW_BOUND`) with the proportions of the source, as BGRA bytes. A system without such a picture answers "not supported"; that is the default of the interface. The client asks through `Calls::share_preview`, which runs the request in a task of the runtime of the client, as the list of sources (ADR 0733). The synthetic capture of the tests gives its two colours, with the left colour changed by the identifier of the source, so that two screens of one size give two pictures. Test: `calls::tests::the_preview_of_a_source_fits_its_bound_and_tells_two_screens_apart` (two screens of 2560 by 1440 give two pictures of 320 by 180 that differ, a window of 800 by 600 gives 240 by 180, a source that is not listed gives "the source is gone", and a system without previews gives "not supported").
- The previews per system: macOS takes one picture with `SCScreenshotManager` (macOS 14 and later; an earlier macOS has no such class and answers "not supported"), with the filter of the stream of a share, the format `420v`, the size of the preview and no cursor, and converts it as a picture of a share. Windows copies the part of the desktop that a monitor shows with GDI (`StretchBlt` from the device context of the desktop), and asks a window to draw itself with `PrintWindow` and `PW_RENDERFULLCONTENT`, so that a window behind other windows gives its own picture; both run on a blocking thread of the runtime. GDI was chosen over one frame of Windows Graphics Capture because a single picture then needs no capture session, no graphics device and no frame pool, and the system draws no capture border around the source while the dialog is open. Linux gives none: the desktop portal shows its own dialog with its own choice (ADR 0751), and the dialog of Junto lists only "Choose a screen with the desktop" and "Choose a window with the desktop" there. The macOS and Windows pictures need a real screen and are checked by a person; the ordinary tests use the synthetic capture.
- A picture crosses the interface as a video buffer of the call library. The macOS buffer reaches the encoder without a copy; the Linux implementation validates and converts the portal's shared-memory pixels (ADR 0751), and the Windows implementation reads the texture of a picture into memory and converts it the same way (ADR 0706).

macOS:

- The binding is `objc2-screen-capture-kit`, which gpui already links. It needs no Swift tool chain at build time.
- A share uses two streams. The stream of the picture has a filter of one screen or of one window, the pixel format `420v`, the size and the frame rate that the person set, and no sound. The stream of the sound has the configuration of the step 0 test, and its filter follows the choice of the sharer: one application (the screen with that application included), all system sound (the screen with nothing excluded), or no stream.
- The two streams are apart, so each choice of sound is possible with each picture, and the sound path is the one that was measured.
- Picture capture requests sRGB and the ITU-R 601 YCbCr matrix explicitly. The untagged I420 conversion and publication path uses that matrix. The display's default color space can differ; using it changed the fixture's green and yellow regions beyond the native test's fixed conversion tolerance. An ordinary configuration test requires both settings independently of the display profile; the native colour tolerance is 12 levels, below the measured 31-level matrix error. That tolerance holds only on a display whose colour space contains the four fixture colours: a display without the fixture blue in its gamut clipped it, and capture returned a green channel 23 levels above the drawn value. The native fixture therefore opens on a screen that shows all four colours without clipping.
- The sound of a shared window, when the person chooses "this application", is the sound of the application that owns the window (ADR 0502).
- A share whose sound stream does not start does not run without sound.
- The system has 10 seconds to answer the request for its lists and the start of each stream. Without an answer the share does not start, and a stream that had started is stopped. A start that completes when nobody waits for it any more stops its stream again.
- The share of a window ends when the system ends its stream, and when the system tells that the stream became inactive, which it does from macOS 15.2 when the window was closed. The stream of the sound ends with the share.
- The size of a picture is the size of the source in pixels, made smaller until it fits the chosen bound (1280 by 720, 1920 by 1080, or no bound), with its proportions and with even sides. A source is not made larger.
- The format of the sound is read from each buffer: 32-bit floats or 16-bit integers, one buffer per channel or one buffer with every channel. Another form gives no sound.

The client itself:

- An application is the client when it has the process identifier of the client, or the same bundle identifier and the same name. The rule and the list of applications are shared with the Windows capture (`share/listing.rs`, ADR 0706). No such application and none of its windows is listed. The second rule covers a second instance of the client: the exclusion of the own sound has no effect on a capture that selects an application of the same responsible application (ADR 0502).
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
