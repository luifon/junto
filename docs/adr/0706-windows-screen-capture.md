# 0706. Windows screen capture with Windows Graphics Capture and process loopback

## Status

Accepted

The tests of `crates/media/tests/windows_capture` ran on the Windows machine of ADR 0905 (Windows 11, build 26200) against a window of a separate fixture program. The sound tests start both choices of sound and receive 48 kHz stereo; they play no tone, so the content of a sound capture is not measured on Windows. The window test of the client `live::windows_share` needs LiveKit and does not run in CI (ADR 0912).

## Context

ADR 0502 requires screen share with sound on Windows and left the sound choices of Windows open until a test had run there. ADR 0736 defines the interface of a capture, and ADR 0751 the delivery of pictures and sound for a capture whose pictures arrive in their own form and whose sound arrives in portions of its own size.

Windows 11 offers Windows Graphics Capture for the picture of one screen or one window. It gives each picture as a Direct3D 11 texture, gives a picture only when the content changes, and tells when the captured window was closed. A desktop application that is not packaged needs no permission for it. The system draws a border around the captured screen or window while the capture runs.

The audio session API of Windows has process loopback (since Windows 10 build 20348): a capture of the sound that one process and its child processes play, or of all sound except that of one process and its child processes. The client plays the sound of a call from its own process.

## Decision

Bindings:

- The capture uses the `windows` crate 0.62.2, the version that gpui and cpal already link, with `windows-core` 0.62.2 for the one COM object of the crate. No new crate version is built. The crate `windows-capture`, which the lock file holds for another dependency, is not used: it links `windows` 0.61, a second version of the bindings.

Lists:

- A screen is named by the number of its device (`\\.\DISPLAY2` is screen 2), and its size is the size of the display mode in pixels.
- A window is named by its handle, which has 32 significant bits on 64-bit Windows. A window is listed when the task bar and Alt+Tab would show it: visible, not hidden by the window manager (another virtual desktop, a suspended application of the store), not minimized, with a title, not a window of the shell (desktop and task bars), each side at least 64 pixels, and either asking for the task bar or neither owned, a tool window nor a window that never takes the focus. Its size is the size of its visible frame in pixels.
- The application of a window is the process of its program; for a frame of an application of the store it is the process of the window inside the frame. An application is identified by its process and the path of its program file in lower case, and named by the description in the version information of the file, or by the file name without its extension.
- The client is its own process and every other process of the same program file with the same name (ADR 0736). None of their windows is listed, and none of them is offered for sound. The applications whose sound can be shared alone are the applications of the listed windows, as on macOS. A person who wants the sound of a program without a window chooses all system sound.
- Every system has both choices of sound: process loopback exists on every Windows 11.

Picture:

- A capture runs on a thread of its own in the multithreaded apartment. It makes a Direct3D 11 device, the capture item of the screen or the window, a free-threaded frame pool of two buffers and a session with the pointer drawn. The border of the system stays.
- The handler of each picture copies the visible part of the picture into a texture of the capture on the graphics card and puts a sign into the slot of ADR 0751. The delivery reads that texture into memory only for a picture that it sends, at the chosen frame rate, and converts it as on Linux. A screen that changes faster than the frame rate costs a copy on the graphics card for each picture and a read into memory only for each sent picture; the last picture of a screen that then stays still is sent.
- A window that changes its size gives pictures of its new size: the pool is made again with that size, and the part of a larger buffer outside the content is not read.
- A screen has no transparent pixels. A fully transparent pixel of a window, at its rounded corners, is sent black, as on Linux.
- The share of a window ends when the system tells that the window was closed; the share of a screen ends when the screen is removed. The sink is told that the source is gone. The client does not start the share again.
- The size of a share is the size of the capture item at the start, made smaller to the chosen bound (ADR 0736).

Sound:

- One application: process loopback of its process and its child processes. All system sound: process loopback of every process except the tree of the client's process. The client's own sound is therefore never in a share, and the sound of a second start of the client is in all system sound, as on macOS.
- The capture asks for 32-bit floats, two channels at 48 kHz, and the system converts to that form. A packet that the system marks as silent gives silence. A packet of another form ends the capture.
- The capture runs on a thread of its own in the multithreaded apartment and waits for each packet with an event. The samples go into the queue of ADR 0751, and the clock of that ADR delivers 10 ms blocks, with silence while no packet arrives.
- A share whose sound does not start does not run without it (ADR 0502).

Start:

- The start checks the request against the lists of the moment: a window that is not listed any more, a screen that is not there, and an application whose process has ended or now runs another program file give "not available any more". The system has 10 seconds to start the picture and the sound.

Unsafe code:

- The crate `junto-media` allows unsafe code in the modules of the Windows capture that call the system. What decides something is in `share/windows/rules.rs`, without unsafe code, and its tests run on every system.

Tests:

- Ordinary tests on every system: the rule of the list of windows, the number of a screen, the handle of a window from its number, the identity and the name of an application, the client, and the frames of a sound packet. The delivery and the conversion of pictures are the tests of ADR 0751, which now run on every system.
- `crates/media/tests/windows_capture` runs in the Windows job of CI. It starts the fixture program `junto-share-fixture`, a binary target of the media crate for tests, which shows a window of four known colours above the other windows without taking the focus. The tests check that the lists name the screen, the window and its application and not the test program; that a share of the window shows the four colours, follows a change of them, and ends with "not available any more" when the window closes, with no picture after the end; that a share of the screen shows the colours at the place of the window and ends without a call of the sink when it is dropped; that a share with the sound of the fixture and a share with all system sound start and give 48 kHz stereo, silent for the fixture; and that a closed window, an unknown screen and the application of an ended process do not start.
- `live::windows_share` of the window tests shares the window of the fixture through the share dialog with the sound of its application, checks the colours that the other person receives, and checks that closing the window ends the share. It needs LiveKit.

## Consequences

- A share shows the yellow border of the system around the shared screen or window. The border is not in the pictures.
- The sound of an application includes the sound of its child processes. A program whose sound is played by a process outside its tree, such as an application of the store whose sound a broker plays, has that sound only in all system sound.
- Whether the capture depends on the volume of the speakers or of the session of the application was not tested.
- The sound tests play no tone: CI must play nothing audible on a machine that a person uses. Exclusion of the client's own sound and inclusion of exactly one application are decided by the system from the process tree and are not measured on Windows. A virtual audio device without speakers on the Windows machine would let a test measure them.
- The client test of the share through the window runs only where LiveKit runs; the Windows job has none (ADR 0912).
- The capture costs one Direct3D device per share.
