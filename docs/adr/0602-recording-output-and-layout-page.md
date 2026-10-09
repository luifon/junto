# 0602. Recording output and the layout page

## Status

Accepted, pending step 0

## Context

A recording must show the shared screen with its sound and must also give the transcription worker separate audio per speaker. The recorder's standard video mixes the sound of every track in the call and has only standard layouts. With several shares at once (ADR 0502), a mix of all tracks contains the sound of shares that the video does not show. A participant who reconnects gets a new track, so the audio of one microphone is not one continuous stream.

## Decision

Video:

- A recording has one MP4 file.
- The project provides its own layout page, which the server serves and the recorder loads (ADR 0001).
- The layout page shows the most recently started share as the main picture, with the sound of that share and all microphones. Cameras and other shares are small tiles.
- The server tells the layout page when the main share changes.

Audio files:

- A recording has one audio file per audio track: the microphone of each participant and each shared-sound track.
- A participant who reconnects gets a new track, so one microphone can have several files.
- Each file is a segment. A segment belongs to one recording and one track and stores its start time.
- The transcript is built on one timeline from those start times (ADR 0603).

Storage and access:

- The files are stored in the file storage of the server (ADR 0001).
- The video and its transcript are posted as one system message in the text chat of the voice channel where the call happened.
- A "Recordings" list per workspace shows all recordings of that workspace.
- Access to a recording follows the permissions of the voice channel.
- Removing a recording or its message needs "manage recordings".

Playing:

- "Open" plays the video in the video player of the operating system.
- The transcript shows the time of each line.
- "Download" offers the video and, separately, the audio files.

## Consequences

- The video contains the sound of one share at a time, the main share. The sound of every other share exists only in its own audio file.
- The project maintains the layout page, and the recorder needs a network path to the server to load it.
- A reconnect during a recording produces an additional audio file for that microphone. The start times place the files on one timeline.
- A person who loses access to the voice channel loses access to its recordings.
- The client has no built-in player in the first version. A built-in recording player that jumps to a transcript line is in the later list of `docs/scope.md`.
- Recordings are included in exports and are deleted with an archived channel and by retention (ADR 0406).
- Step 0 tests the layout page with the main share and its sound, and the separate audio files across one reconnect and one mute (`docs/step-0/README.md`, test 3).
