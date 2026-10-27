# 0604. Recording control on the server: the record, the controller, routes and events

## Status

Accepted

## Context

ADR 0601 decides that a recording is a tracked job with segments and states, that the server starts one recorder job per audio track and one for the composite, and that the server finds a lost recorder from the participant list of the room. ADR 0602 decides the output of the recorder. They leave open how the server stores a recording, when it talks to the recorder, which routes and events exist, who receives the events, and what happens to a recording when its call ends. A request to start a job can take 22 seconds until the LiveKit server answers that no recorder is running (ADR 0601); a request of the API ends after 30 seconds, and a transaction has no effect outside the database (ADR 0009). A voice channel can hide who is inside (ADR 0514), and a recording names the person who started it and the people whose tracks it holds. The transcription worker accepts at most 2000 segments per job (ADR 0610). An event carries no collection that can grow without a bound (ADR 0011).

## Decision

The record:

- A recording row holds the workspace, the voice channel, the call, the room, the state, the person who started it, the language for the transcript, and the stop: its reason, the person who asked for it, and the time.
- A recording refers to its call by the identifier only. The row of a call is deleted when the call ends; the recording stays.
- A voice channel has at most one recording that is starting or runs. A unique index enforces it.
- A segment row holds the recording, the source (composite, microphone, sound of a screen share), the account and the track identifier for an audio track, the job identifier of the recorder, the path that the server asked for, and the file name, start time, end time and length that the recorder reports.
- The segment states are: starting, recording, ended, failed. A failed segment has a reason: no recorder took the request, the recorder refused the request, the request was interrupted, the recorder was lost, or the recorder ended the job with an error. Of a failed segment the stored times state the range that was saved, as far as the recorder reported it.
- The recorder writes the file of an audio track to `<recording>/<segment>` and the composite to the directory `<recording>/<segment>/`, both under the directory of the recordings. Both names are identifiers that the server generates.
- A track identifier or a job identifier of more than 128 characters is not stored: such a track is not recorded, and the start of such a job counts as refused.
- A request that changes a recording locks its row and raises its revision, as a request does with a call (ADR 0510).

States:

- The states of ADR 0601 follow each other in one order: starting, recording, processing, and then one of ready, partly failed or failed.
- A recording is `starting` until the first job of the recorder has started, and `recording` until every job has ended. A stop that is asked for is a field of the recording; the state stays `recording` until the recorder has ended every job.
- A recording that runs with a failed segment stays `recording`. The recording states the number of its segments and of its failed segments, and the segments state what was saved.
- `failed` means that the record knows of no job that ran: with the reason `recorder_unavailable` when the requests to start failed, and with `nothing_recorded` when the recording was stopped before a job started. A segment whose start was interrupted counts as a job that can have run. `ready` and `partly failed` are set after the check of the files (ADR 0602, ADR 0606). A recording whose jobs ran and of which no file is found fails with the reason `files_missing` (ADR 0606).

The recorder client:

- The server reaches the recorder through one trait with five functions: start the composite, start the file of a track, stop a job, name the main share of a composite, and read what the recorder reports about one job.
- The product uses the Egress service of the LiveKit server API over Twirp with JSON bodies, through the same client code as the room service (ADR 0508): `StartRoomCompositeEgress`, `StartTrackEgress`, `StopEgress`, `UpdateLayout` and `ListEgress` with one job identifier. A request carries a token with the grant `roomRecord`.
- The tests use a recorder in memory that behaves as step 0 measured: each job is a participant of the room, a job of a lost recorder is reported as running without end, a stop for it has no answer, and a recorder that starts again resumes nothing.
- A request to start a job has a time limit of 25 seconds. Every other request has a time limit of 5 seconds.
- An answer with the status 408, and a request without an answer, mean "not delivered". An answer with a status of 500 or above means that no recorder took the request.
- The composite is requested as pieces of 2 seconds with a playlist, with the preset 1080p at 30 frames per second, and with the address of the layout page from the setting `JUNTO_RECORDER_LAYOUT_URL`.
- Every request to start a job turns the manifest of the recorder off (`disable_manifest`). With it the recorder wrote a file `<job>.json` with the names of the room and of the track beside each audio file, which the record does not know.
- The main share is named by the identifier of its video track: the layout value is `main:<track identifier>`, and empty when no share is named.

The controller:

- One task of the server runs a pass over every recording that is starting or runs, once per second, and at once after a request that starts or stops a recording and after a verified notification of the media server (ADR 0508). A lost notification is made up for by the next pass.
- A recording has at most one pass at a time. The passes of different recordings run at the same time, so a recorder that answers after 22 seconds delays one recording only.
- A pass reads the participants of the room. It asks the recorder only about the jobs that the participants do not settle. A recording whose jobs all have their participant costs one request per pass.
- A job that records is asked about when it has no participant or its stop was delivered. These reports have a time limit of 5 seconds together. A report that is not there in time, or that the recorder does not give, is left out: the pass decides about that job from the participants alone.
- A job that was declared lost is asked about when a stop of the recording is asked for and the record has closed every segment. No job waits for a request of such a pass, so these reports have no time limit together; each request has its own of 5 seconds.
- The age of a job, and the time since its stop was delivered, count at the moment at which the participants were read, not at the later moment at which the reports are there.
- The pass then changes the record in one serializable transaction (ADR 0009), sends the requests that follow to the recorder outside of any transaction, and stores their answers in a second transaction.
- The identifier of a new segment is generated inside the first transaction, because the segments follow from what that transaction reads. A transaction that runs again generates new ones, and only the requests of the committed run are sent. The identifiers of notes, audit entries and events are generated inside their transaction as everywhere in the server; no caller refers to them.
- A new job is started for the composite when none runs, and for each audio track of a member of the call that has no segment without a failure. A member is recorded from the request to enter, before the admission check has seen the person (ADR 0510), so the first seconds of the track are not lost. A mute changes no track and starts no job.
- The server stops asking for the job of one track, or for the composite, when three requests in a row failed to start it.
- A segment that is still `starting` at the beginning of a pass belongs to a request whose answer was not stored, because the server stopped in between. It fails with the reason "interrupted".
- A participant of the recorder in the room whose identity is no job of the record gets a stop request at every pass. This ends a job whose start answer the server did not store.
- A recording leaves the active states in one place only: when a stop is asked for, the record has closed every segment, the room holds no participant of the recorder, and the recorder was asked in that pass about every job that was declared lost. Until then its reservation and its place among the recordings at one time stay (ADR 0605).
- A participant of the recorder in a room that has no recording which is starting or runs is not allowed there. The admission check (ADR 0509) asks the recorder to stop it, at every check of the room. This covers a job that started after its recording had failed.

Lost jobs, in addition to ADR 0601:

- A job that the recorder reports as ended is ended, with the file result of the report, also when it has no participant.
- A job about which the recorder reports nothing is decided by the rule of ADR 0601 alone.
- A job whose stop was delivered has 20 seconds to report its end before it can be lost. After a loss of the private link a job that had ended was reported as active for 16 seconds.
- A job that still has its participant 20 seconds after its stop was delivered gets the stop again.
- The server sends a stop for a job that it declares lost. A job that was declared lost while its recorder lived then ends, so two jobs do not record the same source. When the stop has no answer and the participant of the job is in the room again, the job gets the stop at every pass until it has left.
- Before a recording leaves the active states, the server asks the recorder again about every job that it declared lost. A complete file result replaces the declaration, and the segment becomes `ended`. A report that the recorder does not give does not hold the recording back, except in the 20 seconds below.
- A job can be declared lost before the recorder took a stop for it. The transaction of the declaration stores with the segment that the stop of its job is owed. While a stop is owed, each pass sends it, and the recording does not leave the active states. The transaction that stores the answers of a pass ends the owed state of each stop that had an end: a stop that the recorder took, or refused because the job had ended, is stored as delivered, and a stop without an answer was not delivered (ADR 0601) and is not sent for this reason again.
- The owed stop is a column of the segment and not a value in the memory of a pass. A server process that stops between the declaration and the answer to the stop, and a transaction of the answers that fails, leave the stop owed: the next pass, of the same process or of the next one, sends it.
- For a recording whose stop is asked for, a second pass follows the pass of a declaration at once and asks the recorder about the job. When the recorder took that stop, the job has the 20 seconds of a stopped job to report its end: in that time the recording waits unless the recorder reports the end of the job or states that it does not know the job. A stop without an answer gives no such time. A job that is declared lost after the 20 seconds of a stop that the recorder took does not hold the recording back.

Start and stop:

- `PUT /api/v1/channels/{channel_id}/call/recording` starts a recording, with an optional language (`auto`, or a code of two or three lower-case letters). `DELETE` on the same path asks for the stop.
- Both need `record_calls` in the channel, and the caller is in the call: the admission check has seen the seat of the caller in the room and has not seen it absent since (ADR 0509). A caller without that gets `not_in_call`. The person who started a recording has no further right over it. The recording continues when that person leaves or loses the permission.
- The start stores the recording in `starting`, with its reservation of disk space (ADR 0605), and answers with it. It does not wait for the recorder.
- When no job of the first requests starts, the server asks for the stop of the recording at once, with the reason `recorder_lost`, and runs the next pass at once. The recording becomes `failed` with the reason `recorder_unavailable` by the rule above: when its room holds no participant of the recorder. A request that had no answer can have started a job; that job gets its stop first. The client learns both steps from events.
- A start for a channel whose recording is starting or runs answers with that recording and changes nothing. While its stop is asked for, a start gets `conflict`.
- A server without a recorder answers a start with `recorder_unavailable` (503).
- A recording is stopped by a person, by the end of its call, and at the storage reserve (ADR 0605).
- A recording has at most 2000 segments, which is the bound of a transcription job. When a further segment is needed, the recording is stopped instead.
- A recording of which no job runs, and for which the server asks for no further job, is stopped with the reason `recorder_lost`. The indicator does not stay while nothing is saved.
- The transaction that ends a call asks for the stop of its recording. A deleted channel ends its call at the next admission check (ADR 0509), which does the same. The lock, the user limit, the hidden occupants and meeting mode do not change a recording.

Reading:

- `GET /api/v1/workspaces/{workspace_id}/recordings` returns the recordings of the workspace, newest first, in pages as the audit log (ADR 0205). `GET /api/v1/recordings/{recording_id}` returns one recording with its segments.
- A recording is returned to a caller with both `view_channel` and `read_message_history` in its voice channel (ADR 0602, ADR 0304). The same check applies to its files, transcript, transcript corrections, speaker names, a request to transcribe again and removal. An active event connection gives no exception for recording content.
- A recording names the person who started it and the people whose tracks it holds, so it tells who is in its call. While its call runs and hides who is inside (ADR 0514), it is returned only to the people inside: also a recording that has ended while the call continues. Inside is the rule of ADR 0517: the admission check has seen the seat of the account. The call hides who is inside when its voice channel did so at the start of the call. When the call has ended, every caller with `view_channel` and `read_message_history` reads it.
- The audit entries that name a recording (`recording_started`, `recording_stopped`, `recording_removed`, `transcript_requested`) name a person inside its call. The audit log of the workspace returns such an entry by the rule of the recording: while the call runs and hides who is inside, to a reader inside only. A reader outside gets the entries when the call has ended. The entry stores the call of its recording, in a column that the API does not return, so the rule holds after the recording was removed while its call runs. An entry from before that column existed is decided by the row of its recording, and when that row is gone, by the call that the channel had when the entry was written. The entries of moderation in a hidden voice channel are returned as ADR 0510 states.
- A recording of a deleted channel is returned to nobody (ADR 0407). Its rows and files stay until retention removes them (ADR 0406).
- The view of a call (ADR 0510) carries the recording that is starting or runs, so a person who joins later sees the indicator.

Events:

| Event | Recipients |
|---|---|
| `recording_started`, `recording_updated` while the call of the recording runs | Accounts that may see who is inside that call (ADR 0514, ADR 0517) |
| `recording_updated` when the call has ended | Accounts with `view_channel` in the voice channel |

- Each event carries the whole recording without its segments. A client reads the segments from the route of the recording.
- The events are general (ADR 0208). An account in the call receives them at any level (ADR 0510).
- `recording_updated` is recorded when the state changes, when a stop is asked for, and when a segment is added, ends or fails.
- The end of a call that hid who is inside records `recording_updated` for the recordings of that call, so that the people outside learn of them: the one that is starting or runs and the newest others, at most 50 together. A call with more leaves the others to the list.
- A channel that stops hiding who is inside while its call runs changes nothing for the recordings of that call: the call keeps the setting that it started with (ADR 0517), and the people outside read its recordings when it has ended. The first version of this record showed them at the change, which let a person with "manage channel" outside the call read who is inside.
- The indicator of ADR 0601 therefore reaches the people in the call and the members who may see who is in the call. It reaches no other person.
- The reasons of a stop and of a failure are open sets: a client reads a reason that it does not know as `unknown`, as ADR 0209 states for error codes.

Audit and notes:

- The start and the stop of a recording are audit entries about the channel (ADR 0305). The entry of a stop by the server has no acting account and states the reason.
- The server keeps a note for the start, the stop and the first failure of a recording, and for its post when its files are processed (ADR 0606), with the identifier of the recording, for the system lines of the text chat (ADR 0501, ADR 0510).
- A file result that replaces a declaration (above) can leave a recording without a failed segment. The transaction of that pass then deletes the note of the failure and its line in the text chat, and records `message_changed` for the deleted line. The note of a failure therefore exists while a segment of the recording has a failure, and for a recording that is not `ready` after its processing (ADR 0606).

Settings:

- `JUNTO_RECORDINGS_DIR` is the directory of the recordings as the server sees it. `JUNTO_RECORDER_DIR` is the same directory as the recorder sees it; without it the two paths are equal. `JUNTO_RECORDER_LAYOUT_URL` is the address at which the recorder loads the layout page.
- The directory and the address of the layout page are given together and need the settings of the media server (ADR 0508). Without them the server has no recorder.

## Consequences

- No transaction waits for the recorder, and no request of the API waits for it.
- Between the request and the first answer of the recorder the indicator shows `starting`. A recorder that is not running is reported up to 25 seconds after the request.
- The state of a recording is correct after a restart of the server: the passes read the record and the room again. A job that was started twice for one source is ended by the stop for unknown participants of the recorder.
- A restart of the LiveKit server removes the participants of the recorder for some seconds. A composite that is older than 10 seconds can then be declared lost: it gets a stop, a new composite starts, and the first one becomes `ended` when the recording stops. The recording then has two composite segments.
- A person outside a hidden voice channel does not learn from a recording who is inside while the call runs. After the call has ended, every person who sees the channel reads who started its recordings and whose tracks they hold, as the transcript states them (ADR 0603).
- The delivery decides an event of a recording that was recorded while its call ran by the people inside that call (ADR 0517). A person who is in a later call of the channel does not receive it in a replay; the event that the end of the call recorded for the accounts with `view_channel` reaches that person.
- A person in the call with `record_calls` can stop a recording that another person started. A person with `manage_recordings` who is not in the call cannot.
- A recording whose call has ended, and whose jobs the recorder never reports as ended, ends when the participants of the recorder have left the room.
- A recording does not leave the active states while the server cannot read the participants of its room, or while a participant of the recorder stays in the room. It counts among the recordings at one time until then, and its stop is sent again at every pass.
- A request to start a job that had no answer within 25 seconds and started a job later, for a recording that fails, can leave a file of some seconds in the directory of that recording. The record does not know that file.
- A file result that the recorder reports after its recording has left the active states is not read. A job that ended complete, and whose report came later than 20 seconds after its stop, stays a failed segment in the record; the recovery of the files on disk (ADR 0602, ADR 0606) finds its file.
- A job of a lost recorder stays in the room for about 20 seconds (ADR 0601). In that time the server sends it a stop at every pass or check, and each has no answer.
- A recording stays `processing` until its files are checked and its composite is joined (ADR 0606). Its reservation of disk space stays for that time. The transcript has its own state and does not hold the recording back (ADR 0607).
- A private-link loss can leave failed segments even while the recorder process and call media stay alive. On 2026-10-16 a 30-second loss in the two-host form caused recorder connection and deadline errors; the server preserved six validated files and ended the recording `partly_failed` with three failed segments (ADR 0003, ADR 0601). The lost-job and file-recovery rules preserve available output; they promise no uninterrupted recording through a control-path failure.
- The route that serves the layout page (ADR 0602) is not part of this record. The server gets the address of the page as a setting.
- The rule that no migration runs while a recording runs (ADR 0005) is enforced at the start of the server (ADR 0608).
- The requests to the Egress service are tested with their bodies and with the answers that step 0 stored. The server code ran them against the recorder in the recording test of the deployment (ADR 0910): the five calls, a recorder that was killed, and a call that everybody left.
- When the media server closes the room of a call in the second in which the server ends that call, the jobs have no participant before their stop is sent. They are declared lost, get their stop, and become `ended` when the recorder reports their files. The note of a failure that the declaration wrote is deleted with its line in the same run of the controller, some milliseconds later; a client that reads the text chat in between shows the line and removes it. The deployment keeps the room longer than the server keeps the call, so that the two do not meet (ADR 0020).
- A server that stops between a declaration and the answer to its stop leaves the stop owed in the record. The next process sends the stop, holds the recording for it, and asks the recorder about the job after it: the file result of a recorder that lives replaces the declaration. A stop that the first process had sent, and whose answer it did not store, is sent a second time; the recorder refuses a stop for a job that has ended, and that answer counts as delivered.
- A stop that follows a declaration and has no answer in its time limit, although the recorder lives, ends the recording with the failed segment: a stop without an answer counts as not delivered (ADR 0601). A lost recorder gives the same answer, so a repetition of that stop would keep every recording of a lost recorder active for the time of the repetitions. The recovery of the files on disk finds the file of such a job (ADR 0606).
- The last questions about lost jobs have no time limit together (above). A recording with many lost jobs and a recorder that does not answer keeps its place among the recordings at one time for 5 seconds per 8 jobs.
