# 0601. Recording start and the recording job model

## Status

Accepted

## Context

Participants record a call, including a shared screen with its sound. A recording runs in the recorder, a separate component that the server controls and that reports by notifications, and those notifications can be lost. During one recording, tracks appear and end: a participant reconnects, or a screen share starts. A recording can fail in part while other parts are saved. Participants must know that a recording runs.

Step 0 tested what the server can learn from the recorder (`docs/step-0/README.md`, test 3):

- After a restart or a crash of the recorder, the recorder's job list kept reporting every job of the lost recorder as active, with no end time and no error. This did not change in 35 minutes.
- A recorder that starts again resumes nothing.
- The recorder is in the room as one participant per job. That participant left the room's participant list 19.4 to 21.6 seconds after the recorder was killed.
- A stop request for a job of a lost recorder answered with a timeout after 3.0 seconds. A stop request answered in the same way while the private link of the two-host form was down and the recorder was alive.
- A participant who reconnects gets new tracks with new identifiers. The file of the old track ends complete, and the recorder does not continue with the new track.
- A muted microphone does not end its file. The file continues with silence.
- A recorder that was short of CPU reported a complete recording with no error, and the file had holes in its sound.

## Decision

Start:

- A recording starts only when a participant with the "record calls" permission starts it.
- Every participant sees an indicator while a recording runs.
- The server checks and reserves disk space before a recording starts (ADR 0006).
- The server waits longer than 22 seconds for the answer to a start request. When no recorder is running, the LiveKit server answers with an error only after 22 seconds. The client then shows that the recorder is not available. The request of the client does not wait for that answer: the recording is `starting` until then, and the client learns the result from an event (ADR 0604).

Third parties:

- The indicator reaches only the people in the call, and the members who may see who is in the call (ADR 0604). It does not reach a person outside the server who appears in a shared window.
- Whether people outside the call who appear in a shared window may be recorded is a legal question for the operator of the server.

Tracked job:

- A recording is a tracked job. The server keeps one record per recording, with its segments and a state.
- The states are: starting, recording, processing, and the final states ready, partly failed and failed. A recording that runs with a failed segment stays in the state recording (ADR 0604).
- A call, a recording and a segment each have their own identifier.
- A segment stores the recording, the participant, the source, the track identifier, the job identifier of the recorder, and the start time, end time, length and file name that the recorder reports for the file.
- A failure appears in the call header and in the text chat of the voice channel, with the time ranges that were saved for each part.

Tracks:

- The server starts one recorder job for each audio track, and a new job with a new segment for each audio track that appears later. A reconnect of a participant and a restart of the LiveKit server both produce new tracks.
- The server finds new tracks by reading the participant list of the room at short intervals. With an interval of one second, the new file missed the first 0.7 to 0.8 seconds of the new track.
- A mute starts no new segment.

Stop:

- The server ends every recorder job with a stop request when the recording ends. Without it, the job of a track runs until the track ends, and the composite runs until 20 seconds after the room is empty.
- A stop request that answers with a timeout was not delivered. The server decides from the detection rule below whether the recorder is lost, and repeats the stop when the recorder is alive.

Detection of a lost recorder:

- The server does not use the status in the recorder's job list to decide whether a job is alive.
- At short intervals and after its own restart, the server reads the participant list of each room with a recording. A job that was started more than 10 seconds ago and has no recorder participant with its identifier is lost.
- On the host of the recorder, two local signals show the loss earlier: the health port of the recorder does not answer, or the newest segment of the composite is older than three segment lengths.
- For a lost job, the server marks the segment as failed with the time range that was saved, recovers the files that are on disk (ADR 0602), and starts new jobs when the call still runs.
- A job that the recorder reports as ended is not lost, and a job whose stop was delivered has 20 seconds to report its end (ADR 0604).
- The end time of a segment comes from the file result that the recorder reports, not from the time at which the server saw the change. After a loss of the private link, a job that had ended was reported as active until 16 seconds after the link returned.

Result:

- A status "complete" of the recorder does not prove a complete file. The server checks every finished file (ADR 0602). A file that fails the check makes the recording partly failed.

Recorder restarts:

- Before a planned upgrade or restart, the operator stops recordings through the client and waits for them to finish, as described in `docs/install.md`. There is no automatic drain command. The recorder has a two-minute stop grace period to finish its files after a stop signal; this does not prove that every recording finished. Restore stops the server, worker and recorder together before replacing stored data (ADR 0005), and does not preserve recordings made after the backup.

## Consequences

- No recording starts automatically, and no recording starts without an indicator for the participants.
- The state of a recording is correct after a lost notification and after a restart of the server, of the LiveKit server or of the recorder, because the server compares its records with the participant list of the room and with the files on disk. It does not depend on the recorder's job list.
- A lost recorder is detected 20 to 25 seconds after its loss from the participant list alone, and within about 6 seconds with the local signals. The rule with the local signals follows from measured times and was not run as code.
- After a crash of the recorder, the composite has a gap of about 9 seconds and each audio track a gap of about 6 seconds, with a recorder that starts again by its restart policy. What was recorded before the crash is kept, except the last seconds (ADR 0602).
- A restart of the LiveKit server loses only the seconds of the restart. The audio files end and new ones start for the new tracks, and the composite continues with a period without sound: 10 seconds in the measured case.
- A recording with a failed segment keeps its other segments, and the participants see which time ranges exist.
- A private-link loss can fail recorder jobs while call media continues. On 2026-10-16 a 30-second loss in the deployed two-host form ended with six validated files and three failed segments, and the recording was `partly_failed` (ADR 0003). Preserving and checking available output and starting replacement jobs do not guarantee an uninterrupted recording (ADR 0604, ADR 0606).
- A recording does not start when disk space cannot be reserved or when the limit on concurrent recordings is reached, and a running recording is stopped in order when free space reaches the reserve (ADR 0006).
- No database migration runs while a recording runs, and the recorder is not replaced while a recording runs (ADR 0005).
- Recording start and stop are written to the audit log (ADR 0303) and to the text chat of the voice channel (ADR 0501).
- Direct-message calls are not recorded (ADR 0501).
- Not tested: notifications of the LiveKit server (webhooks) as a second way to find new tracks, a restart of Redis during a recording, a microphone block by a moderator, which may end the track and its file, and a mute much longer than 20 seconds.
