# 0608. Recording downloads, removal of a recording, and upgrades during a recording

## Status

Accepted

## Context

"Download" offers the video of a recording and, separately, its audio files; access follows the permissions of the voice channel; removing a recording needs `manage_recordings` (ADR 0602). The records do not state the routes, how a large file is sent, or how the rows and the files of a removed recording go together. A video of one hour has about 600 MB. A transaction has no effect outside the database (ADR 0009), and a backup needs the files that it lists until it has finished (ADR 0005). ADR 0005 decides that the server does not migrate while a recording runs, and ADR 0604 left that rule without code.

## Decision

Downloads:

- `GET /api/v1/recordings/{recording_id}/segments/{segment_id}/file` returns the file of one segment: the video of a composite as `video/mp4`, or an audio track as `audio/ogg`.
- The route follows the rule of the recording (ADR 0604). A segment without a file, an unknown segment and a recording that the caller may not read get the same `not_found`.
- A request with `Range: bytes=<first>-<last>`, `bytes=<first>-` or `bytes=-<count>` gets that part with the status 206 and `Content-Range`. A range outside the file gets `range_not_satisfiable` (416) with `Content-Range: bytes */<length>`. A header of another form, and a header with several ranges, gets the whole file.
- The response states `Content-Length`, `Accept-Ranges: bytes`, a file name for saving, and `X-Content-Type-Options: nosniff`.
- The file is opened without following a link (ADR 0606). A link in the place of a file gets `not_found`.
- The server sends at most 32 files at one time. A further request gets `busy` with `Retry-After: 1`.
- A download whose client takes no bytes for 60 seconds is ended.

Removal:

- `DELETE /api/v1/recordings/{recording_id}` removes a recording. The caller may read the recording and holds `manage_recordings` in its voice channel.
- A recording that is starting, runs or is processed gets `conflict`. A recording whose transcript is built can be removed; the process that runs the worker ends it at its next heartbeat (ADR 0607).
- One transaction deletes the rows of the recording, its segments and its transcript, deletes the note of its post, records the audit entry `recording_removed` and the event `recording_removed`, and stores the directory of the recording, and the directories `rescued/<job>/` of its lost jobs, in a queue of removals.
- The notes of the start and the stop of the recording stay, without the recording.
- After the transaction, the task that processes recordings deletes each directory of the queue and then its row. A directory that cannot be deleted stays in the queue and is tried again after 5 minutes. A link is deleted and not followed, and a directory is deleted through descriptors, so that no link above it leads elsewhere (ADR 0606). A row leaves the queue only when its directory is gone.
- The directories `rescued/<job>/` of a processed recording go through the same queue (ADR 0606).

Upgrades:

- At its start, before it applies a migration, the server reads which migrations the database has. When one waits, and a recording is starting or runs, the server does not start and names the reason.
- The host command `junto-server check-upgrade` prints the number of recordings that are starting or run, and ends with the code 3 when there is one.
- A recording whose files are processed, or whose transcript is built, does not hold an upgrade back: both are repeated after a start (ADR 0606, ADR 0607).

## Consequences

- A client reads a large file in parts and continues a download that was interrupted.
- The video player of the operating system cannot send a session token. The client downloads the file and opens the copy (ADR 0602).
- The files of a removed recording are deleted at once after the transaction, and later when the directory cannot be deleted or the server stops in between. The queue is where a running backup will hold a deletion back (ADR 0005).
- A recording of a deleted channel is read by nobody (ADR 0604) and can therefore not be removed through the API. Its rows and files stay until retention is built (ADR 0406).
- An administrator who stops the server during a recording and starts a version with a migration gets a server that does not start. The earlier version is started again, the recording is ended, and the upgrade follows. The host command tells this before the stop.
- A recording that starts between the check and the migration is not seen. Only a second server process on the same database can start one in that time.
- The rule covers the migration of the server. That the recorder is not replaced during a recording (ADR 0005) stays a rule for the administrator, with the same host command.
