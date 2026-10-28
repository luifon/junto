# 0502. Screen share with sound

## Status

Accepted, pending step 0

Open: on Windows the capture starts both choices of sound and gives sound in CI, without a measured tone (ADR 0706). On macOS every capture ran from a program that used the permission of the terminal application that started it; a capture with the permission of the product's own application bundle was not run (`docs/step-0/README.md`, test 1). The macOS capture of the product is built (ADR 0736) and was not run by an automated test. ADR 0751 records the Linux product implementation and its verification status.

## Context

Recording a shared window with its sound is the main use of screen sharing in Junto. Each operating system offers different ways to capture sound: the sound of one application, or all system sound. A change from one application's sound to all system sound without the sharer's knowledge could send an unrelated private call to the participants. Microphone processing, noise suppression and echo cancellation in particular, damages audio that contains several voices. The LiveKit Rust SDK does not capture system sound or application sound.

Step 0 tested the capture on macOS and on Linux with PipeWire (`docs/step-0/README.md`, test 1). On both systems the capture of one application contained that application's tone at its source level and no other source, and the capture of all system sound contained every other source and not the capturing program's own tone.

## Decision

Requirement:

- Screen share with sound is required on macOS, Windows and Linux in the first version.

Choice of sound:

- The sharer chooses the sound of a share: this application, all system sound, or none.
- A choice that the operating system cannot provide is not offered.
- Nothing is chosen automatically.
- The sharer and the viewers see which choice is active for as long as the share runs.
- The sound of the Junto application itself is always excluded, so a call does not hear itself.
- Junto does not offer itself in the list of applications.

Several shares:

- Several people can share at once, and one person can run up to three shares at once, for example two screens, or a screen and a window. Each share has its own choice of sound.
- Three is the limit because each share costs the sharer one capture and one encoder and costs every viewer one more received picture track; a fourth share of one person is refused by the client and by the server.
- Each viewer picks which share to watch, as between the shares of two people, and hears the sound of that share only, also when the other share is a share of the same person.
- A viewer can mute the sound of a share, or change its volume, and keep the picture.
- Sound is separated per application, not per window and not per browser tab of one application: no system offers the sound of one window or one tab. Two meetings in two applications can be shared at once, each with its own sound. Two meetings or two tabs inside one application share the sound of that application. A browser tab with only its own sound needs a browser extension, a separate project.
- The server states the number of shares of one person in each answer that gives a place in a call. A server from before several shares states none, and its end of a share without a slot ends every share, so the client runs one share there (ADR 0510).
- The sharer sees and hears who watches, as in Discord: the device of a person reports the share of another person that it watches (`PUT /api/v1/call/watching` with the account and the slot of the share, or none; the own share, a person outside the call and a slot from `MAX_SHARES` on are refused), the server keeps it on the member of the call and states it on the participant (`watching`), and the header of the own share draws "watched by N" with the names in its tooltip; a person who starts or stops watching plays the viewer sounds of ADR 0707 for the sharer, and a viewer who leaves the call plays the sound of a person who leaves. A new share of a person in a slot ends every watch of that slot on the server, so that the viewers of an earlier share are not counted for the new one; a change of the sound of a running share is no new share. The media engine reports the watched share as it reports the state of the person, again after a new seat, when an event of the own participant states another watch (the server ended it), and when events were lost. Tests: `calls_state::the_share_that_a_person_watches_is_stated_on_the_participant` (with the end of a watch at a new share), `calls::tests::a_value_that_the_server_states_otherwise_is_sent_again`, `state::call_sounds::tests::a_person_who_starts_or_stops_watching_the_own_share_plays_a_sound`, and the live test of a share in the window, in which the device of the administrator watches the share of the window: the window states the viewer and plays the sound of a viewer.

A watch reset or a lost-event indication received during a report is retained. Once the request ends, the reporter reconciles the wanted watch by sending it again if necessary, under its usual spacing and one-request limit. A successful HTTP response does not erase an intervening invalidation: the server may have committed the report before it reset the watch. Tests cover both a reset and a reload before the response.

Track:

- Shared sound is its own audio track.
- It is sent at a higher bitrate, without noise suppression and without echo cancellation.

Capture:

- Capture of system sound and of application sound is the project's own code, with one implementation per operating system (ADR 0701).

macOS:

- The capture uses ScreenCaptureKit. Its audio settings exist from macOS 13.0, so screen share with sound needs macOS 13 or later. Versions before 26 are not tested (ADR 0705).
- Choices offered: this application, all system sound, none.
- "This application" lists only applications that have a window, because the system lists no others. A program without a window is captured only by "all system sound". The sound of an application includes the sound of its helper and child processes. A share of one window carries the sound of the application that owns the window; the system has no sound setting per window.
- Own sound is excluded by the setting of the capture that excludes the current process. The system applies it to the application that it holds responsible for the capturing process and to that application's child processes. Junto therefore plays the sound of a call from its own process or from a child process.
- When the chosen application belongs to the same responsible application as the capturing process, the exclusion has no effect and the capture contains the program's own sound at full level. This is the reason why Junto is not in its own list.
- The capture does not depend on the output volume or on the mute state: the captured level was the same at volume 0 and muted. A sharer can keep the speakers silent.
- The capture delivers 48 kHz stereo in buffers of 20 ms, about 90 ms after playback. The client reads the format from each buffer, because a request for 44.1 kHz was delivered as 48 kHz.
- The permission is "Screen & System Audio Recording". It belongs to the application that the system holds responsible for the process, not to the binary. Without it the system returns an error, not silence, and its dialog has no button that grants the permission. The client checks the permission before a share and sends the person to System Settings.

Linux:

- Sound capture uses PipeWire directly. Junto creates one capture stream and links the output ports of the chosen playback streams to it. It keeps the links up to date from the events of the PipeWire registry, so a stream that starts later is linked and a removed stream is dropped. No session-manager policy, portal or external tool selects these sound links. Picture capture uses the desktop portal and its restricted PipeWire connection (ADR 0751).
- Choices offered: this application, all system sound, none.
- "This application" lists the applications that play sound at that moment, from the clients of the playback streams. On Wayland an application cannot read which process owns the window of another application, so the person chooses the application from this list also for a share of one window. This follows from the Wayland protocol and was not run.
- An application is identified by the application identifier of its sandbox when PipeWire reports one, and otherwise by its binary name and its application name. The process identifier is not used: a sandboxed application reports the identifier inside its sandbox, two sandboxed applications reported the same value, and a rule that used it excluded the wrong application. The node identifier is not used either, because PipeWire reuses it.
- "All system sound" links every playback stream except Junto's own, and skips nodes that belong to a link group (loopback and filter nodes). Without that rule, sound that passes through such a node was captured twice.
- Own sound is excluded by the client of the capture connection and by the process identifier that the PipeWire daemon reads from the socket. Junto plays call sound through the same PipeWire connection, or marks its own nodes with a property, because a stream that is played through the PulseAudio layer carries the identifier of that layer.
- The block size is not constant: 256 to 2048 frames were observed, and it changed during a capture. The capture callback does not run while no captured stream runs, so the sender inserts silence from a clock.
- The first block of a stream that starts during a capture can be missing (1 of 8 starts, 43 ms).
- A program that opens a sound device directly, without PipeWire, has no stream in the graph, so it is not captured. This case and JACK clients were not tested.

Windows:

- Sound capture uses process loopback of the audio session API (ADR 0706). Picture capture uses Windows Graphics Capture.
- Choices offered: this application, all system sound, none.
- "This application" lists the applications that have a window, as on macOS. The sound of an application includes the sound of its child processes. A share of one window carries the sound of the application that owns the window.
- Own sound is excluded from all system sound by the capture of every process except the tree of the client's process. Junto therefore plays the sound of a call from its own process or from a child process, and the applications that Junto opens for a link or a file are started by the Windows shell, outside that tree.
- The sound of an application that started Junto, directly or through other processes, includes the sound of Junto. It is not offered alone, and a share of one of its windows offers all system sound or no sound and states why (ADR 0706). When the process list cannot be read, no application's sound is offered alone. An application above a creator of Junto that has ended is offered; whether its sound includes Junto is open (ADR 0706, Consequences).

## Consequences

- The set of sound choices differs between systems. On macOS and Windows "this application" covers applications with a window; on Linux it covers applications that play sound at that moment.
- A sharer who starts a share makes one explicit choice of sound each time, including the choice "none". Two shares of one person with "all system sound" send the same sound twice; a viewer hears one of them, the one of the watched share.
- The project maintains three implementations of sound capture.
- A shared-sound track uses more bandwidth than a microphone track, because of its higher bitrate.
- The recorder writes shared sound as a separate audio file (ADR 0602), and the transcript labels its lines as shared audio (ADR 0603).
- On macOS the permission is kept across updates only with a stable code identity (ADR 0704).
- The Linux capture was tested with PipeWire 1.6.8 and one session manager in an X11 session, and with a client that PipeWire classified as a Flatpak sandbox. A Wayland session, a real Flatpak build and an AppImage build were not tested. The capture uses no interface of the display server.
