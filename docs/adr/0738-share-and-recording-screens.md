# 0738. The screens of a share and of the recording of a call

## Status

Accepted

## Context

The approved design draws a call with a share (view B), two shares (view E), the Settings tab with a share part and a view part (view B), and the recording in the header of the call with its failure (views B and F). It draws no dialog in which a person chooses what to share. ADR 0502 requires one explicit choice of sound for each share; the design has a stored switch "Include system audio". ADR 0601 and ADR 0604 decide that a participant with "record calls" starts and stops a recording, that every person in the call sees an indicator while it runs, and that the start answers at once and the result arrives by event. ADR 0719 keeps a recording in the state of a server only while it runs. The client holds the roles of a workspace and no override of a channel (ADR 0719). This version has no camera.

## Decision

Starting a share:

- The share button of the strip of the call opens a dialog. It lists the screens with their size and the windows with their application and size, and has one control for the sound: no sound, all system sound, or one application. With a window, "this application" is the application of that window; with a screen, the person chooses the application from a list.
- Nothing is chosen when the dialog opens. "Share" works when a picture and a sound are chosen. The sound "one application" names an application through the picture, so another picture ends that choice and the sound is asked again; "no sound" and "all system sound" hold for every picture. The stored switch "Include system audio" of the design is not built (ADR 0502).
- Without the permission of the system the dialog states how the person gives it (ADR 0736). On a system without a capture the button is disabled and says so.
- While the share runs, the button has its "on" form, and a press ends the share. A share that ended by itself is told by a notice in the notice area of the shown view until the person closes it.
- The dialog shows no preview pictures: a preview needs a capture before the person has chosen.

The stage with a share:

- A line names the sharer and the sound ("with system audio", "with the sound of one application", "without sound"), and at its end what is shared, its size and its frame rate. The sharer reads "You are sharing".
- With two or more shares a bar lists them as cards with a small picture, the name ("Carla's screen", "Bruno's window") and "Watching" or "Watch". A press on a card watches that share. The line then reads "You watch …" with what the person hears, and "The sound follows the share you watch."
- The line tells when the sound of the own share is closed by a microphone block, and when the viewer turned the sound of the share off.
- The picture is drawn inside the stage with its proportions. Until the first picture has arrived the stage says so.
- Under the picture, a strip shows the people as tiles of the size of the design. With "Hide people without video" on, the strip shows no person without a camera and states how many it hides. The setting is off at first, because this version has no camera and it would hide every person.
- Without a share the stage shows the people as large tiles (ADR 0735).

Settings tab of the call:

- "Your screen share": the resolution (720p, 1080p, source) and the frame rate (15, 30, 60) of the next share.
- "Your view": "Hide people without video", and the volume of the sound of the watched share.
- They are stored in `voice.json` (ADR 0710).

Recording:

- The header of the voice channel of the call shows, after its title: "Record" when no recording runs; the mark "REC" with "starting…" while the server asks the recorder; the mark with the time since the recorder started, "visible to everyone", and "Stop" while it runs; "The recording stops" on that button after its end was asked for.
- A person whose roles do not hold "record calls" sees "Record" disabled with the statement of the missing permission, and the mark of a recording without "Stop". The server decides each request, because an override of the channel can differ from the roles; a refusal is told by a notice.
- A recording is started with the language `auto`.
- When a recording of the call ends without a person asking for it, or with a part or all of it not saved, the header shows "Recording failed" or "Recording stopped", "Details" with what happened, the time range that the server has, and what the person can do, and "Record" for a new one. The application reads this from the events of the server, for the call of this device, and keeps it until the person asks for a new recording, a new recording starts, or the call ends. The answer to a request changes only the statement about that request: the event of a failure can arrive before the answer to the request that started the recording. An answer that arrives after its call has ended tells nothing.
- A person outside the call sees the mark without a time.

Structure:

- The application reads every change of the state of a server by itself (the notices of ADR 0719), beside the state, for what the state does not keep.
- "Details" opens and closes with its button, closes with a press elsewhere and with Escape.
- The window derives its view again only after the state of the application told a change. A picture of a share draws the window up to 30 times a second and does not derive.
- An image that the window drew is released when the next picture replaces it.

The notice area is the fixed scrolling area at the end of a call stage, the area above a message list and under its direct-call controls, or the part below the actions of a state panel (ADR 0730).

## Consequences

- A person makes at least three choices for each share: the picture, the sound, and "Share".
- The share part of the Settings tab holds no sound setting.
- A person without "record calls" in the roles who has it by an override of the channel sees "Record" disabled. This holds until the client reads the overrides of a channel.
- A recording that failed before the device entered the call is not told to it: the events from before the entry are not delivered again.
- The time of a recording is computed from the time of the server and the clock of the machine, as the time of a call is (ADR 0735).
- The recording in the chat of its voice channel (view G) and the list of recordings need text messages and follow with them.
- New direct dependency of the desktop client: `image`, in the version that gpui uses, to give gpui the picture of a share.
