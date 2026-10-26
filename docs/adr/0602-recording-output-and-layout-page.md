# 0602. Recording output and the layout page

## Status

Accepted

## Context

A recording must show the shared screen with its sound and must also give the transcription worker separate audio per speaker. The recorder's standard video mixes the sound of every track in the call and has only standard layouts. With several shares at once (ADR 0502), a mix of all tracks contains the sound of shares that the video does not show. A participant who reconnects gets a new track, so the audio of one microphone is not one continuous stream.

Step 0 tested the recorder on one machine and in the two-host form of ADR 0003 (`docs/step-0/README.md`, test 3). The measured facts that decide this record:

- The sound of the composite is what the layout page plays in the recorder's browser. A page that subscribed to every track kept playing a share's sound at full level after that share was no longer the main one.
- A composite that is written as one MP4 file is lost completely when the recorder is killed: the partial file has no index and does not play. With segments of 2 seconds, every finished segment survived.
- The recorder writes each file in its own temporary directory and moves it to the output path when the job ends. The start script of the recorder image deletes that directory at every start.
- The start time that the recorder reports for a composite is 1.49 to 1.96 seconds after the beginning of its content, and the value differs between recordings.
- The recorder reported a complete recording with no error while it was short of CPU, and the file had 21.7 seconds of holes in its sound.

## Decision

Video:

- A recording has one MP4 file, the composite.
- The project provides its own layout page, which the server serves and the recorder loads (ADR 0001).
- The layout page shows the most recently started share as the main picture, with the sound of that share and all microphones. Cameras and other shares are small tiles.
- The layout page subscribes only to the tracks that it plays or shows: the cameras, the shares, the microphones and the audio of the main share. It does not subscribe to the audio of a share that is not the main one, also when that share belongs to the person of the main share: a person can run several shares (ADR 0502), and the page takes the sound track of the main person whose name states the slot of the main share (`share-sound` is slot 0, `share-sound/1` slot 1; ADR 0737). Test: the browser scenario "two shares of one person: the main share plays the sound of its own slot".
- The layout page requests the lowest quality layer for the small tiles. A page that subscribed to every camera at full quality made every publisher of a call with 16 cameras send its highest layer, and the recorder used its whole CPU limit.
- The server names the main share: in the start request, and with a layout update of the running job whenever the main share changes. The main share is the share that started last among the shares of `call_shares` whose picture track the media server lists, found by the slot in the name of the track (ADR 0510). A new share in the slot of a share that ended has the start of its own report, also when the device did not tell the end in between, so it is the share that started last. Test: `recordings_control::a_new_share_in_a_reused_slot_is_the_main_share`. It sends the current value again after the recorder reconnects and after a restart of the LiveKit server. The page's own order of shares is used only when no share is named.

How the composite is written:

- The recorder writes the composite as segments of 2 seconds with a playlist, not as one MP4 file.
- After the stop, the server joins the segments into one MP4 file without encoding again, and deletes the segments after the MP4 passed the check below.
- After a crash of the recorder, the playlist has no end marker, and a join that reads such a playlist does not end. The server joins the segments that are on disk without reading the playlist (ADR 0606).
- When the recorder was lost during a recording, the recording continues in a new composite (ADR 0601). Each part becomes its own MP4 file (ADR 0606).

Audio files:

- A recording has one audio file per audio track: the microphone of each participant and each shared-sound track.
- An audio file is an Ogg file with Opus. The recorder encodes the track again, to stereo at about 128 kbit/s. No tested setting stores the packets as they were sent.
- A participant who reconnects gets a new track, so one microphone can have several files. The file of the old track ends complete, and the server starts a new file for the new track (ADR 0601).
- A mute is silence inside the same file. The file has no gap in its timestamps.
- Each file is a segment. A segment belongs to one recording and one track and stores its start time.
- The temporary directory of the recorder is a directory on a mounted volume of the host. A start script of the deployment saves the partial files from it, to the directory `rescued/<job>/` below the directory of the recordings (ADR 0606), before the start script of the image deletes them. A partial Ogg file plays.

Timeline:

- The transcript is built on one timeline from the start times of the audio files (ADR 0603).
- An audio file is placed at the start time that the recorder reports for the file, minus 0.047 seconds. The start time of the job is not used.
- The composite is placed by its end: its first sample is at the reported end time of the file, minus its media length, minus 0.135 seconds.
- A composite without an end time, after a crash of the recorder, is placed by its sound: the shift at which its loudness agrees best with one microphone file.
- Both constants belong to the tested recorder version. They are measured again when the recorder version changes.

Check of a finished file:

- The server checks every finished file before the recording becomes ready:
  - every stream exists, covers the length of the file and has no hole in its packet timestamps;
  - the composite is not silent in a range in which a microphone file of the same time has sound; a file of digital silence is stated and is no defect, because a muted microphone gives one (ADR 0606);
  - the key frames of the picture are not dark and not of one colour;
  - the media length agrees with the wall-clock length of the recording.
- A file that fails makes the recording partly failed (ADR 0601). The programs, the thresholds and the final state are in ADR 0606.

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
- Loss at a crash of the recorder, measured: 4.2 to 4.5 seconds of the composite at 1080p, and 1.0 to 3.0 seconds of each audio file. With one MP4 file as output the loss was the whole composite.
- Timeline, measured: all 24 audio files were 41 to 56 ms late against their reported start, so the rule places them within 10 ms. The rule for the composite placed it within -15 to +26 ms in 9 recordings. Placement by sound was within -38 to +4 ms of the audio file.
- The delay of a track inside the composite can change during a recording. This was observed once, on a host that was loaded by other work: 0.3 to 0.8 seconds more for about a minute. Transcript lines that are shown against the video are wrong by that amount in such a range.
- Cost of the segments, measured: 6.0 to 6.5 % more disk space than the MP4 of the same encoder, no difference in the CPU use of the recorder, and 0.25 to 0.40 seconds to join 130 seconds of 1080p. The segments and the MP4 exist together until the segments are deleted (ADR 0006).
- CPU and memory of the recorder for one recording, measured on an x86-64 host: a 1080p composite at 30 frames per second with five audio files used 2.5 cores on average and 2.9 at the highest sample, and 1.4 GiB, with no other load on the host. With other work on the host it used 3.9 cores on average. One audio file alone used 0.08 core and 88 MB. On an arm64 machine a 720p composite used 0.65 core on average and 2.5 at its peak.
- The recorder decides from configured CPU costs whether it accepts a request. With its default costs the second audio file of a recording started about 15 seconds late. The deployment sets lower costs that match the measured use.
- Cost of the check, measured: 3.3 to 4.0 seconds for a 1080p file of 130 seconds, and 80 seconds for a file of 61 minutes that was made by repeating one recording. The check found 6 of 6 defective files and passed every good file. It treats only a composite without the sound of a microphone, and dark or one-colour key frames, as defects of the content, so it does not find sound that is present and wrong.
- A person who downloads an audio file gets a copy that the recorder encoded again, not the stream that was sent.
- A person who loses access to the voice channel loses access to its recordings.
- The client has no built-in player in the first version. A built-in recording player that jumps to a transcript line is in the later list of `docs/scope.md`.
- Recordings are included in exports and are deleted with an archived channel and by retention (ADR 0406).
- Not tested: recordings longer than 4 minutes, so drift between the files over an hour is not known; real screen content and speech; a composite with many tiles.
