# junto-server

The Junto server: accounts, device keys, profiles, invites, roles, channels, the audit log, the event stream, the server side of calls, their recordings with files and transcripts, and the public API. The design is in [`docs/adr/`](../../docs/adr/README.md); ADR 0008 describes this crate.

## Run it locally

The server needs PostgreSQL; it is tested with PostgreSQL 17. This starts one in Docker and the server next to it:

```sh
docker run -d --name junto-local-postgres -e POSTGRES_HOST_AUTH_METHOD=trust -p 127.0.0.1:5432:5432 postgres:17-alpine
JUNTO_DATABASE_URL=postgres://postgres@127.0.0.1:5432/postgres cargo run -p junto-server
```

The first start applies the migrations, creates the default workspace with one text channel and one voice channel, and prints a one-time owner code on standard output. The first device that presents the code becomes the server owner. Each start without an owner prints a new code.

`GET http://127.0.0.1:8080/api/server` returns the name of the server and the API versions it supports. `GET http://127.0.0.1:8080/api/v1/health` answers 200 while the server reaches its database and 503 otherwise, and its body states whether the server reaches its media server and how many call rooms keep failing their check (ADR 0012).

A device signs the address that it connected to, and the server accepts a login only for one of its own public origins (ADR 0117). A server that listens on a loopback address accepts `http://localhost:<port>`, `http://127.0.0.1:<port>` and `http://[::1]:<port>` without a setting. A server that other machines reach needs `JUNTO_PUBLIC_ORIGINS`, and does not start without it.

Calls need a LiveKit server. This starts one in development mode, with the key `devkey` and the secret `secret`, and the server with it:

```sh
docker run -d --name junto-local-livekit -p 127.0.0.1:7880:7880 -p 127.0.0.1:7881:7881 -p 127.0.0.1:7882:7882/udp \
    -e LIVEKIT_CONFIG='rtc: {tcp_port: 7881, udp_port: 7882, use_external_ip: false, node_ip: 127.0.0.1}' \
    livekit/livekit-server:v1.13.9 --dev --bind 0.0.0.0
JUNTO_DATABASE_URL=postgres://postgres@127.0.0.1:5432/postgres \
    JUNTO_LIVEKIT_API_URL=http://127.0.0.1:7880 JUNTO_LIVEKIT_URL=ws://127.0.0.1:7880 \
    JUNTO_LIVEKIT_API_KEY=devkey JUNTO_LIVEKIT_API_SECRET=secret cargo run -p junto-server
```

Without the four LiveKit settings the server runs, and a request to join a call gets `media_unavailable`.

## Monitoring

Set `JUNTO_OTEL_ENDPOINT` to the base URL of an administrator-operated OpenTelemetry
collector, for example `http://collector.example.test:4318`. Unset or empty disables
export; the server opens no monitoring connection. The URL accepts HTTP or HTTPS and
an optional path prefix, but no credentials, query or fragment. Use HTTP only on a
private network; HTTPS verifies the collector against the system trust store. The
exporter uses the configured destination directly, without proxies or redirects.
Standard `OTEL_*` exporter settings are ignored, including headers and compression.
Compose passes the endpoint to the server; it does not install a collector.

For an internal CA, set `JUNTO_OTEL_CA_FILE` to a readable PEM CA bundle. These
certificates supplement the system trust store. Compose passes this path; mount the
file read-only in an override file, for example:

```yaml
services:
  server:
    volumes:
      - ./collector-ca.pem:/run/collector-ca.pem:ro
```

Set `JUNTO_OTEL_CA_FILE=/run/collector-ca.pem` in the deployment environment. An
unreadable or invalid bundle prevents startup when export is enabled, with a message
that names the setting without its value. Export disabled means the file is not read.

The server sends OTLP/HTTP JSON to `<base>/v1/traces` and `<base>/v1/metrics` every
five seconds. Each HTTP request that enters the middleware produces one server span, one increment of
`http.server.request.count`, and one observation of `http.server.request.duration`
in seconds. Duration ends when the response headers are ready; it excludes streaming
the body and the lifetime of an upgraded WebSocket. Server errors mark the span as an
error. Attributes contain only the standard HTTP method (other methods are `_OTHER`),
the route template (`unmatched` for an unknown route), and the response status.
If the request future is cancelled before response headers, both metrics are still
recorded and the span has error status. The fixed `error.type=cancelled` replaces the
response-status attribute.
The only resource attribute is the fixed `service.name=junto-server`.

No request or response content, identifiers, names, raw paths, queries, headers,
keys, network addresses or incoming trace context are exported. Existing logs stay
on standard error; the owner code stays on standard output (ADR 0008).

Export runs on background threads. At most 1024 finished spans wait in a queue,
with batches of 256; a full queue drops spans. Each metric retains at most 4096
attribute combinations, then combines further values in an overflow series.
That series carries only `otel.metric.overflow=true`.
An export attempt, including the collector's response, has a two-second limit and is
not retried. A failed export loses
its spans; cumulative metrics remain available at the next collection. Graceful
shutdown attempts a final export. A collector outage does not delay HTTP requests.
Export failures and queue drops produce warnings on standard error, limited to one
per minute per signal and warning kind. Warnings contain no destination or collector
response; queue warnings give the number of dropped spans since the previous warning.

## Event stream

A logged-in device receives the changes of the server over one WebSocket connection (ADR 0107).

1. `GET /api/v1/snapshot` returns the account, its workspaces, and the roles, categories and channels that it may see, with a cursor. It also carries the calls that the account may see, and what waits for the account in calls: invitations for it and by it, moves that name it, and its own requests to enter. Each workspace of the snapshot states `permissions`, the workspace permissions that the account holds there, and `channel_permissions`, one entry for each channel in which a channel override gives the account another list; a channel without an entry has the list of the workspace (ADR 0310). `GET /api/v1/channels/{channel_id}/permissions` states the list for one channel, an archived one included. A client reads what its account may do from these lists and loads a snapshot at `permissions_changed`.
2. The device opens `GET /api/v1/events` as a WebSocket with its session token in the `Authorization` header. The server sends `hello`.
3. The device sends `{"type": "subscribe", "level": "full", "cursor": <cursor of the snapshot>}`. The level `background` delivers control events, direct messages, mentions and call invites, with access checked at delivery.
4. The server sends the events of each change that the account may receive as one message `{"type": "change", "position": <position after the change>, "events": [{"id": <position>, "at": <time>, "event": {...}}, ...]}`, in the order of the positions. The device applies every event of the message and then keeps `position` as its cursor; an event with an `id` at or below its cursor is a repeat.
5. After a disconnection the device subscribes with its cursor. The server replays up to 1000 events that the account may receive. When more wait, or the cursor is not of this log, the server answers `resync`. The device then discards everything that it holds of this server, also what the snapshot does not carry, such as invites and members, and starts again at step 1 (ADR 0207).

Events are written in the transaction of the change that they describe and get their position at the commit (ADR 0010). Each connection reads them from the database and decides with the permission function, immediately before it sends them, whether the account may receive them; a decision is valid for one second (ADR 0011). A client message is one unfragmented frame of at most 4096 bytes. A removal, a ban, a logout and a device removal close the affected connections with the message `ended`. The events and their recipients are in ADR 0208, the limits of a connection in ADR 0116.

## Calls

`call_started`, `own_call_changed.call` and snapshot `own_call` carry the hide setting
retained at call creation. `GET /api/v1/call` also returns `call.inside`: until it becomes
true, the media connection has not yet been observed by admission and inside-only actions
return `not_in_call`. Reload after the own participant-joined event; a completed move
announces its destination own call before that event.

The server decides who is in a call; media goes from the clients to LiveKit and never through the server (ADR 0501 to ADR 0514).

1. A device asks for its place: `POST /api/v1/channels/{channel_id}/call/join` for a voice channel, or `POST /api/v1/direct-calls/{account_id}/join` for a direct message. The server checks the access with the permission function, then the lock and the user limit, and records the place: the account has a seat. A second request for the same seat returns a new token and does not move the end of a seat that has not connected. A request with `?renews=<identity>` renews that seat only: when the device can no longer use it, the answer is `seat_ended`, with the reason in `error.left`, and no seat is made or taken (ADR 0520).
2. The answer carries the address of the media server, the identity of the participant and a join token for one room that is valid for 60 seconds. The device connects to LiveKit with it.
3. The admission check compares the participants of the rooms with the record at the start and every 2 seconds. After each request that changes a call or ends an access, and after each notification of LiveKit that names a room, it checks the changed rooms at once. It removes whoever is not allowed, sets what each participant may publish, and records who is in each call. An account is inside a call from the check that sees its seat in the room (ADR 0517): only then it is shown to others, receives the requests to enter, and acts in the call. A removed person who holds a usable token can be back until the next check (ADR 0509).
4. The events of a call go to the accounts that may see who is inside it; a call that hides who is inside shows the people outside only that the channel is in use (ADR 0514). A call keeps the setting `hides_occupants` that its voice channel had when the call began, and states it in its own field `hides_occupants`; a change of the channel applies to the next call. A connection whose account has a seat in a call receives the events of that call at any level.
5. For a call that hides who is inside, the answer of a join and `GET /api/v1/call` carry no participant, no lock, no meeting and no recording until the account is inside. The two answers also state `meeting_grant`: a microphone block of the account ended as a grant of meeting mode, and the device has not reported an open microphone since (ADR 0521); it is absent in the same case. The device reads `GET /api/v1/call` when `call_participant_joined` names its own account.

The full check takes the rooms in turn, 50 per part; with more calls than that a room is checked less often than every 2 seconds, as ADR 0509 states.

While the server does not reach LiveKit, nobody is let into a call. After the return the changed rooms are checked at once and every other room in its turn. A failure of one room does not stop the check of the others; that room lets nobody in until it is checked again.

`DELETE /api/v1/channels/{channel_id}/call/participants/{account_id}/hand` lowers the raised hand of another person: the person who runs meeting mode does this, and a moderator with `mute_members` (ADR 0516).

Two accounts have a direct-message call only while they share a workspace or one of them administers the server (ADR 0307). One account rings another at most 3 times in 10 minutes and not for 5 minutes after a decline, and an account that muted the conversation or is in its quiet time is not rung; the caller gets the same events and the same count of rings in both cases (ADR 0515). A direct-message call writes system lines into the conversation of the two accounts: `direct_call_started` at the ring, `direct_call_not_answered` or `direct_call_declined` at its end, and `direct_call_ended` with `duration_seconds` after a call that both accounts were inside. A call that was not answered counts as an unread message for the account that was called and gives it a notice of the kind `missed_call`; the lines are the same for an account that muted the conversation or is in its quiet time (ADR 0518).

The server reaches the API of LiveKit (`JUNTO_LIVEKIT_API_URL`) without TLS and refuses an `https://` address at the start. Put that address on a private network (ADR 0508).

LiveKit sends its notifications to `POST /internal/media/notifications`. Configure that address as the webhook of LiveKit, with the same API key. The route accepts only a notification that LiveKit signed; it is not part of the public API, and the entry host does not need to forward it.

## Recordings

A person in the call of a voice channel who holds `record_calls` records it (ADR 0601, ADR 0604). The recorder is LiveKit Egress; the server reaches it through the API of LiveKit.

1. `PUT /api/v1/channels/{channel_id}/call/recording` stores the recording in the state `starting`, with a reservation of disk space, and answers with it. `DELETE` on the same path asks for its stop.
2. The controller of recordings compares each recording with the participants of its room once per second, and at once after a request and after a notification of LiveKit. It starts one job for the composite and one for each audio track, a new one for each track that appears later, and stops every job when the recording ends. A job that is older than 10 seconds and has no participant in the room is lost: its segment fails, and a new job starts while the call runs.
3. The events `recording_started` and `recording_updated` carry the recording without its segments. While the call of the recording runs they go to the accounts that may see who is inside; when the call has ended, to the accounts that see the voice channel. `GET /api/v1/recordings/{recording_id}` returns the segments, and `GET /api/v1/workspaces/{workspace_id}/recordings` lists the recordings.
4. A recording ends when a person stops it, when its call ends, when the free disk space reaches the reserve, and at 2000 segments.

After its jobs have ended a recording has the state `processing` (ADR 0606). One task then takes each recording in turn:

1. It finds the file of each segment below `JUNTO_RECORDINGS_DIR`. The audio file of a lost job is taken from `rescued/<job>/<segment>.ogg`, where the start script of the recorder saves it. Maintenance deletes an entry below `rescued/` that is the job of no segment when it is older than 24 hours.
2. It joins the pieces of each composite into `<recording>/<segment>.mp4` with `ffmpeg`, without encoding again.
3. It checks each file with `ffprobe` and `ffmpeg`, which read the file from the server and get no path of it: the streams, their timestamps, the length against the length that the recorder reported, the key frames of the picture, and the sound of the composite against the microphone files.
4. The recording becomes `ready`, `partly_failed` or, without any file, `failed`. Its reservation of disk space ends, and `recording_updated` is recorded. Each segment then states its file and the problems that the check found.

A server with a recorder needs `ffmpeg` and `ffprobe` on its path, or `JUNTO_FFMPEG` and `JUNTO_FFPROBE`, and does not start without them.

`GET /api/v1/recordings/{recording_id}/segments/{segment_id}/file` returns the video or an audio file, whole or as one byte range (`Range: bytes=...`). The server sends 32 files at one time and 4 of them to one account; a further request gets `busy`. A download that runs is decided again every 5 seconds and ends when its account may no longer read the file; the downloads of attachments and of exports follow the same rule (ADR 0608). `DELETE /api/v1/recordings/{recording_id}` removes a recording with its files and its transcript; it needs `manage_recordings` (ADR 0608).

## Transcripts

With `JUNTO_TRANSCRIBE_COMMAND`, the server hands each processed recording to the transcription worker of [`workers/transcription`](../../workers/transcription/README.md) (ADR 0607, ADR 0611).

1. The server writes the job file into a new directory `<recording>/transcription/<run>/` and runs `<command> run <job file> --out <transcript file> --audio-root <directory of the recording>` as a child process in a process group of its own. The process gets `PATH` and the `JUNTO_TRANSCRIBE_*` variables of the worker, and nothing else of the environment of the server. With `JUNTO_TRANSCRIBE_BATCH_SIZE`, the server adds `--batch-size <number>` to the command.
2. One job runs at a time for all server processes, and none starts while a call is recorded. A run has a time limit of 13 hours. A run that fails is repeated twice, with a wait.
3. The server accepts a transcript only when it agrees with its job, and stores its lines. The transcript of a recording is `waiting`, `running`, `ready`, `partial` or `failed`; the recording carries that state, and each change is a `recording_updated` event.
4. `GET /api/v1/recordings/{recording_id}/transcript` returns the state and the result of each audio segment, `.../transcript/lines` the lines in pages, and `.../transcript/text` the transcript as plain text. `PUT .../transcript/lines/{line}` corrects a line and `PUT .../transcript/speakers/{number}` names a voice of shared sound; every person who reads the recording does both, each change names its author, and the text of the worker stays. The events `transcript_line_corrected` and `transcript_speaker_renamed` state the line and the label that changed and carry no text and no name; a client reads the line from `.../transcript/lines?after=<line - 1>&limit=1` and the label from `.../transcript`. `POST .../transcript` asks for a failed transcript again and needs `manage_recordings`.
5. `GET` and `PUT /api/v1/workspaces/{workspace_id}/glossary` read and replace the glossary of a workspace: at most 200 names and terms, most important first, which the speech model receives. `PUT` needs `manage_glossary`.

The settings for a worker that is installed beside the server:

```sh
JUNTO_TRANSCRIBE_COMMAND=/usr/local/bin/junto-transcribe JUNTO_TRANSCRIBE_MODELS=/srv/junto/models
```

The worker reads the audio files of a recording and writes only into the directory of its run. Give its user no write access to the audio files.

The storage guard (ADR 0605) keeps 5 GiB free, reserves 8 GiB for each recording and records 2 calls at one time. A start without that space gets `storage_low`, and a third recording gets `recordings_full`. The health route states `storage_low`.

The recorder writes below `JUNTO_RECORDINGS_DIR`: the file of an audio track to `<recording>/<segment>.ogg` and the composite as pieces with a playlist to `<recording>/<segment>/`. The server writes the videos and the transcripts into the same directories, so the user of the server needs write access to what the recorder creates. The recorder loads the layout page from `JUNTO_RECORDER_LAYOUT_URL`. The server serves the page at `/recording-layout/` from the directory in `JUNTO_RECORDER_LAYOUT_DIR`: build it with `npm ci --ignore-scripts && npm run build` in [`recording-layout/`](../../recording-layout/README.md), put `recording-layout/dist` beside the server, and give the recorder the address of `/recording-layout/` on the private network (ADR 0615). The route asks for no credential and is not part of the public API; the log record of a request has the path without the query, so the room token of the recorder is not logged. A join token carries the display name of its account, which the page shows.

## Access tokens

An administrator creates an access token for another program with a session (ADR 0203, ADR 0210):

```sh
curl -X POST https://chat.example.test/api/v1/access-tokens \
    -H "Authorization: Bearer $SESSION_TOKEN" -H 'content-type: application/json' \
    -d '{"name": "setup", "actions": ["read_structure", "manage_structure"], "expires_at": "2027-01-01T00:00:00Z"}'
```

The answer carries the token (`jat_...`) once; the server stores its hash. A token has an expiry at most 366 days ahead and a set of actions: `read_structure`, `manage_structure`, `manage_members`, `read_audit_log`. The table `REACH` in `src/tokens.rs` lists the routes of each action; every other route refuses an access token with `forbidden`. A request with a token gives and takes no role of an account and lifts no ban; these routes need a session (ADR 0213). A request with a token is decided with the roles that the creator has at that time, and never as the server owner. `GET /api/v1/access-tokens` lists the tokens and `DELETE /api/v1/access-tokens/{id}` revokes one. A ban, a removal and a recovery of the creator delete the creator's tokens. Each audit entry of a request with a token names the token in `access_token_id`.

## Setup tool

`junto-setup` (crate [`crates/setup`](../setup)) writes the structure of a server to a file and applies a file to a server, through the public API with an access token (ADR 0204, ADR 0211, ADR 0212):

```sh
export JUNTO_SERVER=https://chat.example.test JUNTO_ACCESS_TOKEN=jat_...
junto-setup export --out structure.json     # workspaces, roles, categories, channels, overrides
junto-setup plan structure.json             # prints what apply would change; changes nothing
junto-setup apply structure.json            # all changes or none
junto-setup plan structure.json --out plan.json && junto-setup apply --plan plan.json
```

The token is never an argument: it comes from `JUNTO_ACCESS_TOKEN` or from `--token-file`. The tool sends it over `https`, or over `http` to a loopback address. The file format is in [`docs/api/structure-file.json`](../../docs/api/structure-file.json). An entry is matched by its `id`; an entry or an optional field that the file does not hold is kept; only what the file lists under `delete` is deleted: `channels`, `categories`, `roles`, `server_roles` and `workspaces`. A new entry gets a new UUID from its author; the server generates the identifier of the object and keeps the identifier of the file for later files. A workspace with `"default": true` and a role with `"everyone": true` stand for the default workspace and for the everyone role of the server that the file is applied to, so the export of one server can be applied to another.

`GET /api/v1/structure` returns the document with its revision, and `POST /api/v1/structure/changes` applies at most 200 changes in one transaction. A request that names another revision than the current one gets `conflict`, and so does a plan after any change of the structure. Both routes need the administrator permission.

The tests run the commands of the tool against a server in the test process. One more tier runs the binary against two running servers with empty databases; it is ignored in the plain run:

```sh
cargo build -p junto-setup
JUNTO_TEST_SETUP_SOURCE='http://127.0.0.1:8081|<owner code>' \
JUNTO_TEST_SETUP_TARGET='http://127.0.0.1:8082|<owner code>' \
    cargo test -p junto-server --test api setup_real:: -- --ignored
```

## Host commands

These commands work on the database directly and are not part of the API (ADR 0114, ADR 0010, ADR 0018). Each reads `JUNTO_DATABASE_URL` and can run while the server runs; `health` and `schema-version` read no database.

```sh
junto-server assign-owner <account-id>   # make an active account the server owner
junto-server issue-owner-code            # print a one-time code that returns the owner to their account
junto-server new-event-epoch             # make every client load a snapshot
junto-server check-upgrade               # tell whether a recording runs; exit code 3 while one does
junto-server backup-begin                # hold file deletions back; print the identifier of a backup
junto-server backup-files <backup-id>    # after the copy of the database: print the directories to copy
junto-server backup-end <backup-id>      # the backup has copied its files; exit code 1 when it has expired
junto-server restored                    # after a restore: new event epoch, no export, no backup
junto-server schema-version              # print the newest migration of this binary
junto-server unnamed-files [--remove]    # list, or remove, the directories that no row names
junto-server health                      # exit code 0 while the server of this host answers 200
```

`health` reads `JUNTO_LISTEN_ADDRESS` only and asks `GET /api/v1/health`; it is the health check of a container (ADR 0012).

A backup copies the database and the files with the programs of the deployment (ADR 0018). `backup-begin` prints `backup <id> <expiry>`. From then on, and for 24 hours at most, the server deletes no directory of an attachment or of a recording. Copy the database. Then run `backup-files <id>` with the settings of the server: it prints `file <directory>` for each directory that the backup copies and `skip <directory> <reason>` for each one that is still written. Copy the directories of the `file` lines, then run `backup-end <id>`. After the database and the files of a backup were put in place, run `restored` before the server starts.

`backup-files` prints no line for a recording of the state `failed`, which has no file. It prints the `file` line of a `ready` or `partly_failed` recording also when the directory is absent; treat an absent directory of a `file` line as a loss. `backup-end` ends with the exit code 1 and changes nothing for a backup that has expired (`the backup <id> expired at <time>`) and for an identifier of no backup; a backup that was ended is unknown afterwards. Discard a backup whose `backup-end` failed: the server may have deleted files before they were copied.

`schema-version` prints one line, `schema <number>`, where the number is the newest migration that the binary applies, in decimal digits without leading zeros; the exit code is 0, and the command reads no setting. A server that runs has this schema. Store the line with a backup, and compare it with the line of the installed binary before a restore replaces anything. `restored` compares the newest migration of the database with that number: when they differ it prints `the database has the schema <found>, and this version of the server needs the schema <needed>`, changes nothing and ends with the exit code 1.

`unnamed-files` reads the directories of `JUNTO_ATTACHMENTS_DIR`, `JUNTO_RECORDINGS_DIR` and `JUNTO_EXPORTS_DIR` that are set, in one full scan each; run it with the settings of the server. A restore and the way back from an upgrade can leave directories that the database does not name. The command prints one line for each directory whose name is an identifier and that no attachment, recording, queued removal or export names:

- `unnamed <kind>/<identifier>` without `--remove`: nothing is changed.
- `removed <kind>/<identifier>` with `--remove`: the directory was deleted.
- `skip <kind>/<identifier> changed_recently` in both forms: the directory was changed in the last 24 hours and stays.

The kind is `attachments`, `recordings` or `exports`. The exit code is 0, also when no line is printed. With `--remove`, the command removes nothing and ends with the exit code 1 while a backup holds deletions back. In both forms it changes nothing and ends with the exit code 1 when no directory setting is given, when two settings name the same directory, and when the database has another schema than the binary. It examines no entry with another name (`rescued/` is swept by maintenance) and no file or link. Run `unnamed-files --remove` before `backup-begin` of a regular backup, so that each backup removes what a restore or the way back from an upgrade left more than 24 hours before.

Before a version of the server applies a migration, it copies the database to `<name>_before_<newest applied migration>` on the same PostgreSQL server and keeps that one copy. The copy needs that no other program is connected to the database and that the database user may create a database; otherwise the server does not start. `JUNTO_MIGRATION_COPY=none` turns the copy off. To go back: stop the server, rename the database, rename the copy to the name of the database, and start the earlier version.

The copy has the comment `junto: copy of the database <name> before a migration`. From the copy until a start has applied every migration, the comment of the database itself begins with the line `junto: an upgrade of this database has not finished. <copy> is the copy from before the upgrade`; a comment that the database had stays after that line. While the database has the line, a start keeps the copy and makes no copy: a migration that failed can have left the database migrated in part. The next upgrade after a finished one replaces the copy. The server creates the copy as `<name>_making_<newest applied migration>` and gives it the name of a copy when it has its comment. Before it creates that database, it writes the line `junto: the server makes the copy <that name>` into the comment of its own database; a start removes a database of that name that a stopped start left, and only one that this line or the comment of a copy states as its own. Another database with that name stops the start.

Run `check-upgrade` before an upgrade of the server or of the recorder. A server whose new version changes the database does not start while a recording is starting or runs (ADR 0005, ADR 0608).

Run `new-event-epoch` after the restore of a backup, before the server accepts clients. The clients hold cursors of events that the restored database does not have; the new epoch makes the server refuse those cursors.

## Settings

| Variable | Default | Meaning |
|---|---|---|
| `JUNTO_DATABASE_URL` | required | PostgreSQL connection URL |
| `JUNTO_LISTEN_ADDRESS` | `127.0.0.1:8080` | Address and port of the HTTP listener |
| `JUNTO_PUBLIC_ORIGINS` | the loopback origins of the port, when the server listens on a loopback address; required otherwise | Comma-separated origins at which clients reach the server, for example `https://chat.example.test`; `http` is accepted for a loopback host only (ADR 0117) |
| `JUNTO_SERVER_NAME` | `Junto` | Name of the server and of its default workspace; read at the first start only |
| `JUNTO_TRUSTED_PROXIES` | empty | Comma-separated IP addresses or networks (`172.31.240.0/24`) of the entry hosts whose `X-Forwarded-For` header the server trusts |
| `JUNTO_EXPORTS_DIR` | none | Existing absolute directory for the files of exports. Without it the server makes no export (ADR 0411) |
| `JUNTO_MIGRATION_COPY` | `database` | `database`: copy the database before a migration; `none`: do not (ADR 0018) |
| `JUNTO_LOG_FORMAT` | `json` | `json` or `text`; log records go to standard error |
| `JUNTO_LOG_LEVEL` | `info` | `error`, `warn`, `info`, `debug` or `trace` |
| `JUNTO_STOP_WITH_PROCESS` | none | The number of the process that starts the server, such as the process of the tests. The server stops, as on SIGTERM, when that process ends, also when it is killed; on Unix systems it stops at once when that process is not its parent (ADR 0915) |
| `JUNTO_LIVEKIT_API_URL` | none | Address of the LiveKit API on the private network, with `http://` (ADR 0508) |
| `JUNTO_LIVEKIT_URL` | none | Address of LiveKit for clients, with `wss://` or `ws://` |
| `JUNTO_LIVEKIT_API_KEY` | none | API key of LiveKit |
| `JUNTO_LIVEKIT_API_SECRET` | none | Secret of that key; never printed or logged |
| `JUNTO_RECORDINGS_DIR` | none | Directory of the recordings as the server sees it, an absolute path (ADR 0604) |
| `JUNTO_RECORDER_DIR` | the value of `JUNTO_RECORDINGS_DIR` | The same directory as the recorder sees it |
| `JUNTO_RECORDER_LAYOUT_URL` | none | Address at which the recorder loads the layout page, with `http://` or `https://` |
| `JUNTO_RECORDER_LAYOUT_DIR` | none | Absolute directory of the built layout page, with its `index.html`. Without it the server serves no layout page (ADR 0615) |
| `JUNTO_FFMPEG` | `ffmpeg` | The program that joins and reads the files of a recording (ADR 0606) |
| `JUNTO_FFPROBE` | `ffprobe` | The program that reads the packets of a file |
| `JUNTO_TRANSCRIBE_COMMAND` | none | The program of the transcription worker and its first arguments, separated by spaces. Without it the server builds no transcript (ADR 0607) |
| `JUNTO_TRANSCRIBE_SEPARATE_SPEAKERS` | `true` | Whether the voices in shared sound get speaker labels (ADR 0614) |
| `JUNTO_TRANSCRIBE_BATCH_SIZE` | none | The speech clips that the speech model reads at one time, from 1 to 64. The server gives it to each run of the worker as `--batch-size`; without it the worker uses the number of its model. A smaller number needs less memory (ADR 0607) |
| `JUNTO_TRANSCRIBE_MODELS`, `JUNTO_TRANSCRIBE_WHISPER_MODEL`, `JUNTO_TRANSCRIBE_DEVICE`, `JUNTO_TRANSCRIBE_MAX_*` | none | Settings of the worker, which the server passes to its process |

The four LiveKit settings are given together or not at all. The directory of the recordings and the address of the layout page are given together and need the LiveKit settings; without them a request to record gets `recorder_unavailable`. The settings of ffmpeg and of the transcription worker need the directory of the recordings.

## Tests

The integration tests need Docker. The script starts a temporary PostgreSQL, runs the command and removes the container:

```sh
tools/with-test-postgres.sh cargo test --workspace
```

The container has the name `junto-dev-postgres`. Set `JUNTO_TEST_POSTGRES_CONTAINER` to another name to run the tests of two checkouts at the same time. Each test creates its own database. Without the script the integration tests fail with a message that names it. The tests of the event stream open real WebSocket connections to the server; some of them hold all deliveries to fix what waits in the log (ADR 0011). Every transaction of the server is serializable and retried (ADR 0009); the tests of races hold one request before its commit to fix the order. PostgreSQL ends a statement after 10 seconds and a wait for a lock after 5 seconds; the request then gets the error `timeout`. The tests that need no database run alone with:

```sh
cargo test -p junto-authz -p junto-protocol
cargo test -p junto-server --lib --test openapi
```

On a machine where other work compiles, run heavy commands through `tools/serial.sh`, for example `tools/with-test-postgres.sh tools/serial.sh cargo test --workspace`.

The tests of calls have two tiers (ADR 0902). The first tier is part of the run above: it uses a media server in memory that behaves as step 0 measured LiveKit, and each test runs every admission check itself, so the order of things is fixed. The second tier runs against a real LiveKit server in Docker with the participants of `spikes/test-participant`. Its tests are ignored in the plain run:

```sh
(cd spikes/test-participant && ../../tools/serial.sh cargo build --release)
tools/with-test-postgres.sh tools/with-test-livekit.sh tools/serial.sh \
    cargo test -p junto-server --test api live:: -- --ignored
```

The tests of recordings use a recorder in memory that behaves as step 0 measured LiveKit Egress, and each test runs the passes of the controller, the processing and the transcription itself. The recorder in memory writes files that `ffmpeg` generates, so these tests, and `cargo test -p junto-server --lib`, need `ffmpeg` and `ffprobe` with the encoders `libopus` and `libx264` on the path. `deploy/tests/recording.sh` records a call through the deployment with the real recorder (ADR 0910); it is not part of CI.

The tests of the hand-over to the transcription worker run a stand-in script that follows the contract of ADR 0611. One more tier runs the real worker with its development model on generated speech. It is ignored in the plain run and needs the virtual environment, the models and the test speech of `workers/transcription`:

```sh
tools/with-test-postgres.sh tools/serial.sh \
    cargo test -p junto-server --test api transcriber_real:: -- --ignored
```

`tools/with-test-livekit.sh` starts LiveKit 1.13.9 in development mode as the container `junto-dev-livekit` on the local ports 17880 to 17882 and removes it afterwards. LiveKit sends its notifications to `127.0.0.1:17890` on the machine of the tests, where one test listens; on Linux, set `JUNTO_TEST_WEBHOOK_ADDRESS` to an address of the machine that the container reaches. The server has the settings of `deploy/livekit-common.yaml`, as the deployment has (ADR 0904).

The live tier accepts `JUNTO_TEST_PARTICIPANT` as an executable path, so a debug build can
be used: `cargo build --manifest-path spikes/test-participant/Cargo.toml --target-dir target`,
then set the variable to the absolute path of `target/debug/test-participant`.

Unix-mode, symbolic-link and process-group tests are compiled only on Unix. On Windows
the recording-file implementation refuses operations with Unsupported; Windows CI currently
builds server tests but does not run the database tier (ADR 0905). The unsupported-storage implementation and its refusal tests compile on every test platform, including Unix; a Windows runner is still needed to check the complete Windows build. Unix process-group
termination has no Windows equivalent in this implementation.

## Checks

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo deny check
```

## API description

`docs/api/openapi.json` is generated from the routes, `docs/api/events.json` from the message and event types of the event stream (ADR 0206), and `docs/api/structure-file.json` from the types of the structure file (ADR 0212). After a change to a route, a type or an event, write them again and commit them:

```sh
cargo run -p junto-server --bin junto-openapi
```

The test `openapi` fails while a committed file differs from the code.

## Text messages

The test database server starts with `pg_stat_statements` preloaded. The tests that count
statements read it. Its list of entries has a fixed size for all databases together, so the
database of a test records no statement: the test support sets `pg_stat_statements.track`
to `none` for each database that it creates. A test that counts statements starts its server
with `TestServer::start_measured`, which leaves the recording on for that database, and ends
with `require_no_statement_eviction`, which fails when the database server evicted an entry
during the measurement. A temporary database role identifies request statements separately
from the event watcher. The ignored test `statement_statistics::entries_of_the_run` prints
the number of entries when it runs after the suite in the same call of
`tools/with-test-postgres.sh`. The test support deletes the database of a test when the
test ends: the database server holds statistics for each table and index of each database
in a shared memory of fixed size, and the databases of a whole run do not fit into it.

Channels (including voice-channel chat) use `GET` and `POST /api/v1/channels/{id}/messages`.
Two-account conversations are opened with `POST /api/v1/conversations` and use
`/api/v1/conversations/{id}/messages`. A message is fetched, edited and deleted at
`/api/v1/messages/{id}`. Documents are typed JSON; history positions, replies, reactions,
read positions and system messages follow ADR 0408. The snapshot includes unread summaries,
with 100 meaning 100 or more. Use `before_message` or `after_message` to page through private
system lines that share a public position. Each line keeps the audience it had when written.

Set `JUNTO_ATTACHMENTS_DIR` to a dedicated existing absolute directory for attachments.
`JUNTO_ATTACHMENT_BYTES` defaults to 100,000,000 and `JUNTO_ATTACHMENTS_PER_MESSAGE` to 10.
`JUNTO_PICTURE_BYTES`, the largest account picture and server logo, defaults to 2,097,152
(2 MiB, the largest request body); a picture has at most 1024 by 1024 pixels.
Administrators can persist overrides with `PUT /api/v1/attachment-limits`; every authenticated
client can read the effective values with GET.
Start an upload at `POST /api/v1/attachments`, send 1 MiB chunks to its `chunks/{offset}`
route, then complete it after all bytes have arrived. Upload status permits resumption.
The generated API describes the request types. Back up this directory with the database.

Threads use `GET` and `POST /api/v1/messages/{id}/thread`; fork a thread with
`POST /api/v1/messages/{id}/fork`. Each thread has one fork, copied in bounded batches.
Deleting its destination permits another fork; an archived destination keeps the marker.
Occupant hiding is inherited only by voice forks. Authors and named recipients follow
threads; role/everyone mentions count as unread without following. Participants and mention
recipients read or advance their position at `/api/v1/messages/{id}/thread/read`.
`PUT /api/v1/messages/{id}/thread/follow` stores the choice of the caller to follow a thread or
not, and `DELETE` removes the choice; a thread that is not followed has no unread replies and
no thread notices, and a mention still notifies (ADR 0412). A root message carries
`reply_authors`, the first three authors of its replies. `GET /api/v1/channels/{id}/threads`
lists the threads of a channel that the caller follows, read ones included, the one with the
newest reply first (ADR 0413).
The snapshot contains at most 200 unread threads, plus `threads_more` and `threads_next`.
Continue with `GET /api/v1/threads/unread?before=...&limit=...` (maximum 200); unread mentions
of unfollowed threads appear with zero unread replies.
Pin or unpin an authored channel message at
`/api/v1/messages/{id}/pin`, and read pins at `/api/v1/channels/{id}/pins`.
`GET /api/v1/search?q=...` searches current authorized messages and transcript lines. A hit
states the `position` and the `thread_root` of its message, or the `start_ms` of its
transcript line. `GET /api/v1/recordings/{id}/transcript/lines` takes `around=<line>` for the
page with that line in its middle and `before=<line>` for the page before a line.

Read notifications at `/api/v1/notifications` and mark one read with
`PUT /api/v1/notifications/{id}/read`. `PUT /api/v1/notifications/read` marks every notice
that the account reads and that was created at or before `until` in one request, and the devices of the account get
`notifications_read`, with `until` and `read_at`. The stored read boundary also marks notices from transactions that commit later with an earlier timestamp. Personal settings use
`/api/v1/notification-settings/{scope_id}`: the nil UUID names account/server defaults,
and a workspace or channel UUID names its override. Workspace administrator defaults
use `/api/v1/workspaces/{id}/notification-default`: `GET` returns the level or `null` when
none is set, `PUT` sets one and `DELETE` removes it; the snapshot carries it, and a change is
the event `notification_default_changed` with the level. A daily quiet time states
`start_minute`, `end_minute` and `time_zone`, the name of an IANA time zone that the database
knows, and holds while the clock of that zone is inside the window (ADR 0412). Clients apply synchronized do-not-disturb
settings to OS notifications; the server also suppresses notification delivery and direct-call
invitations during quiet periods, including replay. Equal daily quiet endpoints are invalid.
Unread state continues to update. Reading a place marks its
notices read, and maintenance keeps the newest 500 notices per account for at most 90 days.
Cap maintenance visits only recipients of new notices. Edits adding mentions can upgrade
ordinary or thread notices and reset their read state.

Administrators set workspace file retention at `/api/v1/workspaces/{id}/file-retention`
recording retention at `/api/v1/workspaces/{id}/recording-retention`, and direct-message
retention at `/api/v1/direct-message-retention`. A null `days` value
means never, the default. Retention visits places in identifier order, committing bounded
batches for one place at a time. Final recordings include ready, partly_failed and failed.
Message retention and attachment cleanup run in maintenance.
Messages retain a flag when file retention removes their attachments.
The full rules and pagination contracts are in ADR 0409.

## Archive, export and deletions

`PUT /api/v1/channels/{id}/archive` archives a channel and `DELETE` on the same path restores
it; both need `manage_channel` (ADR 0410). An archived channel is in no channel list and not
in the snapshot. The people who see it read it; every change of its content or settings gets
`conflict`, and the call of an archived voice channel ends. `GET
/api/v1/workspaces/{id}/archived-channels` lists the archived channels with the time of their
deletion. `GET` and `PUT /api/v1/workspaces/{id}/archive-period` read and set the period: 7 to
36500 days or `null` for never, 30 by default. Maintenance records the notice
`channel_deletion_due` 7 days before a deletion and deletes the channel 7 days after its
notice. A channel that contains permanent messages is not deleted by the server.

`POST /api/v1/channels/{id}/exports` asks for an export of a channel and needs
`export_channel`; `POST /api/v1/conversations/{id}/exports` asks for an export of a
direct-message conversation of the caller (ADR 0411). An export belongs to its requester:
`GET /api/v1/exports`, `GET` and `DELETE /api/v1/exports/{id}` and `GET
/api/v1/exports/{id}/file` serve that account only, and the download decides the permission
again. The file is a ZIP64 archive with `export.json`, `messages.json`, `messages.html`,
`attachments/` and `recordings/`. It holds what the requester reads of the items that existed
at the request. One task writes one export at a time: it counts the bytes, reserves them
(ADR 0605) and writes in pages. An export is removed 7 days after it finished, when an item of
it is deleted, and with its account or its channel. Set `JUNTO_EXPORTS_DIR`.

`DELETE /api/v1/workspaces/{id}/roles/{role}`, `DELETE /api/v1/server-roles/{role}`, `DELETE
/api/v1/workspaces/{id}/categories/{category}` and `DELETE /api/v1/workspaces/{id}` delete a
role, a category and a workspace (ADR 0308). The channels of a deleted category stay. A
workspace is deleted by an administrator, by the server owner only when a channel of it
contains permanent messages, and never the default workspace.

A deletion of a channel or of a workspace changes rows of the structure in its request. The
content (messages, attachments, recordings, exports) is removed afterwards by one task, in
transactions of at most 500 rows (ADR 0017). The tests run it with
`junto_server::purge::run_once`, the exports with `junto_server::exports::run_once`.


## Role order

Ban and removal accept an optional JSON `reason` of at most 1000 characters without
controls. It is retained only in the audit entry. POST on
`/api/v1/workspaces/{workspace_id}/members/{account_id}/removal` removes only that membership and
its roles, requiring administrator and a strictly higher workspace rank. It ends the
workspace seat and rechecks contact; other memberships and server access stay. In the same
transaction, invites created by the removed account lose this workspace. Single-workspace
invites are revoked; other invites keep their remaining workspaces and emit `invite_changed`
and the audit action `invite_workspaces_changed` (ADRs 0106 and 0111). Server removal keeps
invites valid; a ban cancels them. An invite made by another member still permits return.

PATCH on `/api/v1/channels/{channel_id}/overrides/{role_id}` takes
`{"permission":"mute_members","state":"deny"}`. State is allow, deny or inherit. It
preserves every other permission and applies the grant ceiling to the changed bit.

Roles carry a `position`; larger values are higher, and everyone stays at zero.
The owner stands above all roles. Server actions compare server roles. In a workspace,
administrators compare their highest administrator-granting server roles above workspace
roles; other people use only that workspace's roles. Other server permissions give no
workspace position. Bans, removals and restored roles on unban require a strictly higher
position; administrator does not bypass this check. Role management compares the role,
not the recipient. Voice moderation, moves and content moderation keep ADR 0309's exemptions.

`PATCH /api/v1/server-roles/positions` and
`PATCH /api/v1/workspaces/{workspace_id}/roles/positions` take
`{"positions":[{"role_id":"<uuid>","position":1}]}`. At most 250 named roles move
atomically, to distinct positions from 1 through 1,000,000. Both old and new positions
must be below the actor, and collisions with unnamed roles are refused. Everyone cannot
move. The routes require `manage_structure` for tokens and use the creator's current
position. Taking an unknown role returns `not_found` after visibility checks.

Role lists, role-created/updated events and workspace snapshot roles carry positions.
Holders of `ban_members` can read server roles and receive their events; tokens may use
`read_structure` or `manage_members` for that list. A reorder writes position audit entries
and permission-change notices. Creation writes one audit entry and at most three events
under the 250-role ceiling:
`server_role_positions_shifted` or `workspace_role_positions_shifted` carries the changed
roles in chunks of at most 128 `positions` (each `role_id` and absolute `position`), then
the usual created
event adds the new role at one. The workspace event also names `workspace_id`.
The shift emits no permission-change notices because the relative order is unchanged.

Structure format 2 exports optional relative-order keys. Setup sorts positioned roles
into their existing slots after creations and deletions, preserving unnamed/unpositioned
roles. New unpositioned roles remain at the bottom. Old exports tolerate later client
creations, and repeated apply changes nothing. Format 1 files remain readable; old tools
must be upgraded before reading format 2 exports.

Structure format 3 adds the `glossary` of each workspace, its terms in their order of
priority. Applying a file replaces each glossary that it states with the change
`replace_glossary`, which has the checks, limits, audit entry and event of the glossary
route; an absent `glossary` keeps the glossary. Formats 1 and 2 remain readable; old tools
must be upgraded before reading format 3 exports.

## Audit filters and profiles

Both audit lists accept `action`, `actor_id`, `target_id`, inclusive `since` and exclusive
`until` (RFC 3339). Repeat the same filters with `before` to continue; each page has at most
200 entries. A target is an identifier named by the event or its workspace/channel/call
scope. Filters retain the current channel and hidden-call checks and disclose no total.
Action, actor, time and target lookups have indexes (ADR 0305). A time filter orders by
occurrence time then identifier; its continuation uses the last returned entry under the
same filters and audience. Both lists share a limit of 120 requests per minute per account.

Invite lists include the creator's current retained display name in `created_by_name`;
`invite_created` keeps the name at creation. Only the existing invite readers receive it.
Member lists, `workspace_member_added`, call participants and their events include optional
avatar metadata, without image bytes. Removing the last device returns `last_device` (409).

## Server logo

An administrator with a session uploads an image to `PUT /api/v1/server/logo` or removes it
with `DELETE`. Avatar image bounds, format checks and the profile-change rate limit apply.
The public `GET` returns the image with its media type, hash ETag and `nosniff`; `/api/server`
advertises its path. A change records `server_logo_changed` for active accounts and the audit
action of the same name. The image is public even before joining (ADR 0101). GET uses
`public, no-cache`, supports `If-None-Match` with 304 responses, and has a limit of 120
requests per minute per address. Cross-origin embedding of this public image is permitted.


## Replay retention and admission compatibility

A message and its following notification reserve equal positions regardless of a recipient's
mute or quiet time. They form one retained unit, so the default 5,000-unit log keeps the
original ordinary-message horizon with at most 10,000 rows after maintenance. Suppressed
notices wake nobody, and other notices wake only their eligible recipient connections.

An absent `OwnCallState.inside` means the earlier server did not state admission; only an
explicit false means entering. Migration 0016 renews the event epoch once without changing
historical payloads, so reconnecting clients load a fresh snapshot (ADR 0517).
