# 0607. Transcription hand-over, transcript storage, corrections and the glossary

## Status

Accepted

## Context

The transcription worker reads one job file and writes one transcript file, opens no socket, and runs one job per process (ADR 0610). ADR 0611 fixes the two files and leaves open how the server starts the worker and passes the files. ADR 0610 states that the worker cannot be cancelled except by ending its process, and that the server needs its own time limit. One job can take hours on a CPU, and one GPU serves one job at a time (ADR 0603). The recorder needs CPU cores that no other component takes while a recording runs (ADR 0003). Anyone who can see a recording can correct a line or rename a speaker, each change shows who made it, and the original text stays viewable; each workspace has a glossary (ADR 0603). ADR 0604 states that a recording stays `processing` until the transcript is built. The design of the client shows a finished recording with its video and, beside it, the state of its transcript.

## Decision

State:

- A transcript has its own state: `none`, `waiting`, `running`, `ready`, `partial` or `failed`. The state of a recording is final when its files are processed (ADR 0606) and does not wait for the transcript.
- `ready`: every audio segment of the recording was transcribed whole. `partial`: the transcript covers a part of the audio. `failed`: no transcript was built, with a reason.
- A recording carries the state and the number of lines of its transcript. A change of the state is recorded as `recording_updated`, in the transaction of the change (ADR 0011).

The job:

- When the files of a recording are processed, and at least one audio file has a place on the timeline, the server stores a job in the state `waiting`. A server without a worker stores none.
- The job file follows ADR 0611. The identifiers are those of the server. A participant is the account of the track, with its name; a name that the format refuses is replaced by the identifier of the account. A microphone is the kind `microphone` and the sound of a screen share the kind `shared`. The start of a segment counts in milliseconds from the start of the timeline (ADR 0606).
- The glossary of the workspace is read when the job file is written. A term that the format refuses is left out.
- Speaker separation is asked for when the recording has shared sound, unless `JUNTO_TRANSCRIBE_SEPARATE_SPEAKERS` is `false`.
- An audio segment without a file, or whose file has no place, is not part of the job.

The worker process:

- `JUNTO_TRANSCRIBE_COMMAND` is the program of the worker with its first arguments, separated by spaces. The server adds `run <job file> --out <transcript file> --audio-root <directory of the recording>`.
- Each run has a new directory `<recording>/transcription/<attempt>-<random>/` with the job file, and the worker writes the transcript file into it. A run first deletes the directories of earlier attempts, and at its end deletes its own directory. It never deletes the directory of a later attempt, which belongs to a process that took the job from it. The audio paths of the job are the names of the audio files in the directory of the recording.
- A run is a process group of its own, because the command can be a script that starts the worker. The server signals the group. When a run has ended, in any way, the server ends every process that is left of its group, and waits at most 5 seconds for the log of the run to close. It ends the group before it collects the worker, so the identifier of the group cannot belong to another process by then.
- The process gets `PATH` and the variables of the worker that are set for the server (`JUNTO_TRANSCRIBE_MODELS`, `JUNTO_TRANSCRIBE_WHISPER_MODEL`, `JUNTO_TRANSCRIBE_DEVICE` and the limits of `workers/transcription/README.md`). It gets no other variable.
- `JUNTO_TRANSCRIBE_BATCH_SIZE` is a setting of the server: a whole number from 1 to 64, the speech clips that the speech model reads at one time. The server adds it to each run as the option `--batch-size <number>` after the audio root. The worker reads the batch size from that option only, so the variable is not passed to the process. A server with another value does not start. Without the setting the worker uses the number of its model.
- The log lines of the worker are written to the log of the server. They hold no value of a job (ADR 0610).
- A run has a time limit of 13 hours, one hour above the worker's own limit. At the limit the server sends the stop signal to the group, and ends the group 10 seconds later when the worker still runs.

One job at a time:

- A server process claims a job in one transaction: the job becomes `running` with the identifier of the process and a heartbeat, and its number of attempts grows by one.
- No job is claimed while another job is `running`. This holds for all server processes on one database.
- No job is claimed while a recording is starting or runs. A job that runs is not ended when a recording starts.
- The process renews the heartbeat every 15 seconds. A job whose heartbeat is older than 90 seconds has lost its process: it waits again, or fails with `interrupted` when it has no attempt left.
- A process whose renewal finds the job gone, or taken by another process, ends its worker and stores nothing. A transcript is stored only by the process that holds the job.

Results:

- Exit code 0: the server reads at most 64 MiB of the transcript file, refuses a longer one, and accepts the transcript only when it agrees with the job: the version, the recording, the language and the glossary; the same segments in the same order; every line with the kind and the participant of its segment, in the order of the timeline, with a text of 1 to 4000 characters without control characters; every speaker label listed for a participant with shared sound; every status with its reason and no reason that the contract does not have; the number of lines of each segment. Unknown fields are refused in every part that the server stores. The parts that it does not store, which are the word times, the texts of the reasons, the engine and the timing, are not examined.
- The server stores at most 200 000 lines of one transcript, speaker labels up to `Speaker 10000`, and times up to 8 days: the latest start of a segment in a job plus the longest audio of a job. A transcript beyond one of these is refused.
- A transcript that is refused, an exit code other than 0, 2 and 3, and a worker that does not start are followed by another run after a wait of one minute for each run so far. A job has three runs. Then it fails with `transcript_invalid` or `worker_failed`.
- Exit code 2 fails the job with `job_refused`, exit code 3 with `worker_not_ready`, and the time limit with `time_limit`. No further run follows.
- A segment for which the worker reports more than one second less audio than the file holds is `damaged` with the reason `shorter_than_file` (ADR 0611).
- An audio segment that was not part of the job has the status `not_transcribed` with the reason `no_file` or `no_start_time`. A segment for which no job of the recorder ran is not listed.
- The transcript is `partial` when a segment is `damaged`, `failed` or `not_transcribed`. Missing speaker labels do not make it partial; the segment states them.
- A person with `manage_recordings` asks for the transcript again with `POST /api/v1/recordings/{recording_id}/transcript` when the recording has files and has no transcript or a failed one.

Storage:

- The file of the worker is kept as `<recording>/transcript.json`. It names the models and settings that produced it (ADR 0611). It is written in the transaction that stores the transcript, after the check that the process holds the job, so a process that lost the job writes no file. A transcript whose file cannot be written is not stored, and the run counts as failed.
- The lines are rows: position, start and end, segment, kind, account, number of the speaker label, text and language. Word times stay in the file.
- A correction is a new row with its text, its author and its time. The newest correction is the text that is shown; the text of the worker is never changed. A line keeps at most 100 corrections, and a further one gets `limit_reached`.
- A speaker label is a row with the account whose shared sound it belongs to. A name of a label is a new row with its author and its time; the newest name is shown.

Routes:

- `GET /api/v1/recordings/{recording_id}/transcript` returns the state, the speaker labels with their names, the glossary terms that were used and cut, and the result of each audio segment.
- `GET .../transcript/lines` returns the lines in pages of at most 500, with `after`. A corrected line carries the correction, its author, its time and the text of the worker.
- `PUT .../transcript/lines/{line}` corrects a line with a text of 1 to 2000 characters. `PUT .../transcript/speakers/{number}` names the voice of the label `Speaker <number>` in this recording.
- `GET .../transcript/text` returns the transcript as plain text, one line of text for each line, with its time, its speaker and its newest correction.
- Every route follows the rule of the recording (ADR 0604): the caller sees the voice channel, and while the call of the recording runs in a channel that hides who is inside, the caller is inside. Corrections and renames need no further permission (ADR 0603) and count against the limit of actions of a session.
- `transcript_line_corrected` carries the recording, its workspace, its voice channel and the position of the line. `transcript_speaker_renamed` carries the same three identifiers and the number of the label. Neither carries a text, a name, an author or a time. The event log is append-only (ADR 0010): a text in an event stays readable from the log after its recording was removed (ADR 0608), also for an account that joined later. ADR 0408 keeps message text out of events for the same reason.
- A client that receives one of the two events reads the line from `.../transcript/lines` (`after` is the position before the line, `limit` is 1) or the label from `.../transcript`. Both events go to the recipients of the events of the recording.
- The migration that introduces this rule removes the text, the author and the time from the stored events of corrections, and replaces the label of the stored events of renames by its number.

Glossary:

- `GET /api/v1/workspaces/{workspace_id}/glossary` returns the terms in their order of priority to every member of the workspace. `PUT` on the same path replaces the whole list and needs `manage_glossary`.
- A glossary has at most 200 terms. The server removes the space at the start and at the end of a term. A term then has 1 to 200 characters, no control character, and not the text `<|` or `|>`; no term is listed twice. These are the rules of the job format (ADR 0611).
- The replacement is an audit entry that states the number of terms, and the event `glossary_replaced` names the workspace.
- The worker cuts the list at 200 tokens and reports the terms that it used (ADR 0613). The transcript of a recording states them.

## Consequences

- A recording is `ready` when its video and its audio files can be used. Its transcript follows later: on a CPU, one hour of dense speech takes several hours (ADR 0603).
- A failure of the transcript does not change the state of the recording.
- The server must be able to start the worker as a child process. How a deployment does this, for example with a script that starts the worker container, is not part of this record. A command with a space inside one argument needs such a script.
- The worker reads the directory of the recording as its audio root and writes into `<recording>/transcription/`. A deployment gives it no write access to the audio files.
- A stop of the server ends its worker. The job is run again by the next process after 90 seconds, and that run counts as an attempt. Three stops during one job fail it as `interrupted`.
- A server process that was ended by a signal that it cannot handle leaves a worker that runs to its end. The next process starts a second one after 90 seconds. A server that stops in order ends the group of its worker.
- A script that puts the worker into another process group, or into a container that it does not stop on the stop signal, keeps it out of reach of the server.
- While a call is recorded, no transcript starts. A transcript that runs when a recording starts keeps its CPU cores; the limits of the worker container bound them (ADR 0610).
- A glossary term that the tokenizer of the speech model turns into a control token makes the worker refuse the job (ADR 0613). The transcript fails with `job_refused` until the term is removed and the transcript is asked for again.
- A transcript that was asked for again replaces the failed one. A transcript with lines is not replaced, so no correction is lost.
- Every person who reads a recording can change its transcript. The corrections of a line are kept in the database; the API shows the newest.
- The event of a correction or of a rename states which line or label changed, not what it holds now. A client that shows the transcript sends one request for it. After the removal of a recording, the event log holds the identifiers of the recording and the positions that were changed, and no text of it.
- The lines are rows, so the search of ADR 0405 can read them.
- The tests run a stand-in process that follows the contract. One tier, which is ignored in the plain run, runs the worker of `workers/transcription` with the development model on a short generated speech file.
