# 0601. Recording start and the recording job model

## Status

Accepted, pending step 0

## Context

Participants record a call, including a shared screen with its sound. A recording runs in the recorder, a separate component that the server controls and that reports by notifications, and those notifications can be lost. During one recording, tracks appear and end: a participant reconnects, or a screen share starts. A recording can fail in part while other parts are saved. Participants must know that a recording runs.

## Decision

Start:

- A recording starts only when a participant with the "record calls" permission starts it.
- Every participant sees an indicator while a recording runs.
- The server checks and reserves disk space before a recording starts (ADR 0006).

Third parties:

- The indicator reaches only the people in the call.
- Whether people outside the call who appear in a shared window may be recorded is a legal question for the operator of the server.

Tracked job:

- A recording is a tracked job. The server keeps one record per recording, with its segments and a state.
- The states are: starting, recording, partly failed, failed, processing, ready.
- The server also asks the recorder for its list of jobs, at intervals and after a restart.
- The server starts a new segment when a new track appears.
- A failure appears in the call header and in the text chat of the voice channel, with the time ranges that were saved for each part.
- A call, a recording and a segment each have their own identifier.

## Consequences

- No recording starts automatically, and no recording starts without an indicator for the participants.
- The state of a recording is correct after a lost notification and after a restart of the server or of the recorder, because the server compares its records with the recorder's list of jobs.
- A recording with a failed segment keeps its other segments, and the participants see which time ranges exist.
- A recording does not start when disk space cannot be reserved or when the limit on concurrent recordings is reached, and a running recording is stopped in order when free space reaches the reserve (ADR 0006).
- No database migration runs while a recording runs (ADR 0005).
- Recording start and stop are written to the audit log (ADR 0303) and to the text chat of the voice channel (ADR 0501).
- Direct-message calls are not recorded (ADR 0501).
- Step 0 tests recovery of the job state after the recorder restarts (`docs/step-0/README.md`, test 3).
