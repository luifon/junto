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

## Event stream

A logged-in device receives the changes of the server over one WebSocket connection (ADR 0107).

1. `GET /api/v1/snapshot` returns the account, its workspaces, and the roles, categories and channels that it may see, with a cursor.
2. The device opens `GET /api/v1/events` as a WebSocket with its session token in the `Authorization` header. The server sends `hello`.
3. The device sends `{"type": "subscribe", "level": "full", "cursor": <cursor of the snapshot>}`. The level `background` delivers control events only until direct messages, mentions and call invites exist.
4. The server sends each event that the account may receive as `{"type": "event", "id": <position>, "at": <time>, "event": {...}}`, in the order of the positions. The device keeps the `id` of the last event that it applied; an event with an `id` at or below it is a repeat.
5. After a disconnection the device subscribes with its cursor. The server replays up to 1000 events that the account may receive. When more wait, or the cursor is not of this log, the server answers `resync`. The device then discards everything that it holds of this server, also what the snapshot does not carry, such as invites and members, and starts again at step 1 (ADR 0207).

Events are written in the transaction of the change that they describe and get their position at the commit (ADR 0010). Each connection reads them from the database and decides with the permission function, immediately before it sends them, whether the account may receive them; a decision is valid for one second (ADR 0011). A client message is one unfragmented frame of at most 4096 bytes. A removal, a ban, a logout and a device removal close the affected connections with the message `ended`. The events and their recipients are in ADR 0208, the limits of a connection in ADR 0116.

## Calls

The server decides who is in a call; media goes from the clients to LiveKit and never through the server (ADR 0501 to ADR 0514).

1. A device asks for its place: `POST /api/v1/channels/{channel_id}/call/join` for a voice channel, or `POST /api/v1/direct-calls/{account_id}/join` for a direct message. The server checks the access with the permission function, then the lock and the user limit, and records the place.
2. The answer carries the address of the media server, the identity of the participant and a join token for one room that is valid for 60 seconds. The device connects to LiveKit with it.
3. The admission check compares the participants of the rooms with the record at the start and every 2 seconds. After each request that changes a call or ends an access, and after each notification of LiveKit that names a room, it checks the changed rooms at once. It removes whoever is not allowed, sets what each participant may publish, and records who is in each call. A removed person who holds a usable token can be back until the next check (ADR 0509).
4. The events of a call go to the accounts that may see who is inside its voice channel; a channel that hides who is inside shows the people outside only that it is in use (ADR 0514). A connection whose account is in a call receives the events of that call at any level.

The full check takes the rooms in turn, 50 per part; with more calls than that a room is checked less often than every 2 seconds, as ADR 0509 states.

While the server does not reach LiveKit, nobody is let into a call. After the return the changed rooms are checked at once and every other room in its turn. A failure of one room does not stop the check of the others; that room lets nobody in until it is checked again.

Two accounts have a direct-message call only while they share a workspace or one of them administers the server (ADR 0307). One account rings another at most 3 times in 10 minutes and not for 5 minutes after a decline, and an account that muted the conversation is not rung (ADR 0515).

The server reaches the API of LiveKit (`JUNTO_LIVEKIT_API_URL`) without TLS and refuses an `https://` address at the start. Put that address on a private network (ADR 0508).

LiveKit sends its notifications to `POST /internal/media/notifications`. Configure that address as the webhook of LiveKit, with the same API key. The route accepts only a notification that LiveKit signed; it is not part of the public API, and the entry host does not need to forward it.

## Recordings

A person in the call of a voice channel who holds `record_calls` records it (ADR 0601, ADR 0604). The recorder is LiveKit Egress; the server reaches it through the API of LiveKit.

1. `PUT /api/v1/channels/{channel_id}/call/recording` stores the recording in the state `starting`, with a reservation of disk space, and answers with it. `DELETE` on the same path asks for its stop.
2. The controller of recordings compares each recording with the participants of its room once per second, and at once after a request and after a notification of LiveKit. It starts one job for the composite and one for each audio track, a new one for each track that appears later, and stops every job when the recording ends. A job that is older than 10 seconds and has no participant in the room is lost: its segment fails, and a new job starts while the call runs.
3. The events `recording_started` and `recording_updated` carry the recording without its segments. While the call of the recording runs they go to the accounts that may see who is inside; when the call has ended, to the accounts that see the voice channel. `GET /api/v1/recordings/{recording_id}` returns the segments, and `GET /api/v1/workspaces/{workspace_id}/recordings` lists the recordings.
4. A recording ends when a person stops it, when its call ends, when the free disk space reaches the reserve, and at 2000 segments.

After its jobs have ended a recording has the state `processing` (ADR 0606). One task then takes each recording in turn:

1. It finds the file of each segment below `JUNTO_RECORDINGS_DIR`. The audio file of a lost job is taken from `rescued/<job>/<segment>.ogg`, where the start script of the recorder saves it.
2. It joins the pieces of each composite into `<recording>/<segment>.mp4` with `ffmpeg`, without encoding again.
3. It checks each file with `ffprobe` and `ffmpeg`, which read the file from the server and get no path of it: the streams, their timestamps, the length against the length that the recorder reported, the key frames of the picture, and the sound of the composite against the microphone files.
4. The recording becomes `ready`, `partly_failed` or, without any file, `failed`. Its reservation of disk space ends, and `recording_updated` is recorded. Each segment then states its file and the problems that the check found.

A server with a recorder needs `ffmpeg` and `ffprobe` on its path, or `JUNTO_FFMPEG` and `JUNTO_FFPROBE`, and does not start without them.

`GET /api/v1/recordings/{recording_id}/segments/{segment_id}/file` returns the video or an audio file, whole or as one byte range (`Range: bytes=...`). `DELETE /api/v1/recordings/{recording_id}` removes a recording with its files and its transcript; it needs `manage_recordings` (ADR 0608).

## Transcripts

With `JUNTO_TRANSCRIBE_COMMAND`, the server hands each processed recording to the transcription worker of [`workers/transcription`](../../workers/transcription/README.md) (ADR 0607, ADR 0611).

1. The server writes the job file into a new directory `<recording>/transcription/<run>/` and runs `<command> run <job file> --out <transcript file> --audio-root <directory of the recording>` as a child process in a process group of its own. The process gets `PATH` and the `JUNTO_TRANSCRIBE_*` variables of the worker, and nothing else of the environment of the server.
2. One job runs at a time for all server processes, and none starts while a call is recorded. A run has a time limit of 13 hours. A run that fails is repeated twice, with a wait.
3. The server accepts a transcript only when it agrees with its job, and stores its lines. The transcript of a recording is `waiting`, `running`, `ready`, `partial` or `failed`; the recording carries that state, and each change is a `recording_updated` event.
4. `GET /api/v1/recordings/{recording_id}/transcript` returns the state and the result of each audio segment, `.../transcript/lines` the lines in pages, and `.../transcript/text` the transcript as plain text. `PUT .../transcript/lines/{line}` corrects a line and `PUT .../transcript/speakers/{number}` names a voice of shared sound; every person who reads the recording does both, each change names its author, and the text of the worker stays. `POST .../transcript` asks for a failed transcript again and needs `manage_recordings`.
5. `GET` and `PUT /api/v1/workspaces/{workspace_id}/glossary` read and replace the glossary of a workspace: at most 200 names and terms, most important first, which the speech model receives. `PUT` needs `manage_glossary`.

The settings for a worker that is installed beside the server:

```sh
JUNTO_TRANSCRIBE_COMMAND=/usr/local/bin/junto-transcribe JUNTO_TRANSCRIBE_MODELS=/srv/junto/models
```

The worker reads the audio files of a recording and writes only into the directory of its run. Give its user no write access to the audio files.

The storage guard (ADR 0605) keeps 5 GiB free, reserves 8 GiB for each recording and records 2 calls at one time. A start without that space gets `storage_low`, and a third recording gets `recordings_full`. The health route states `storage_low`.

The recorder writes below `JUNTO_RECORDINGS_DIR`: the file of an audio track to `<recording>/<segment>.ogg` and the composite as pieces with a playlist to `<recording>/<segment>/`. The server writes the videos and the transcripts into the same directories, so the user of the server needs write access to what the recorder creates. The recorder loads the layout page from `JUNTO_RECORDER_LAYOUT_URL`; the server does not serve that page yet.

## Host commands

Four commands work on the database directly and are not part of the API (ADR 0114, ADR 0010). Each reads `JUNTO_DATABASE_URL` and can run while the server runs.

```sh
junto-server assign-owner <account-id>   # make an active account the server owner
junto-server issue-owner-code            # print a one-time code that returns the owner to their account
junto-server new-event-epoch             # make every client load a snapshot
junto-server check-upgrade               # tell whether a recording runs; exit code 3 while one does
```

Run `check-upgrade` before an upgrade of the server or of the recorder. A server whose new version changes the database does not start while a recording is starting or runs (ADR 0005, ADR 0608).

Run `new-event-epoch` after the restore of a backup, before the server accepts clients. The clients hold cursors of events that the restored database does not have; the new epoch makes the server refuse those cursors.

## Settings

| Variable | Default | Meaning |
|---|---|---|
| `JUNTO_DATABASE_URL` | required | PostgreSQL connection URL |
| `JUNTO_LISTEN_ADDRESS` | `127.0.0.1:8080` | Address and port of the HTTP listener |
| `JUNTO_PUBLIC_ORIGINS` | the loopback origins of the port, when the server listens on a loopback address; required otherwise | Comma-separated origins at which clients reach the server, for example `https://chat.example.test`; `http` is accepted for a loopback host only (ADR 0117) |
| `JUNTO_SERVER_NAME` | `Junto` | Name of the server and of its default workspace; read at the first start only |
| `JUNTO_TRUSTED_PROXIES` | empty | Comma-separated IP addresses of the entry hosts whose `X-Forwarded-For` header the server trusts |
| `JUNTO_LOG_FORMAT` | `json` | `json` or `text`; log records go to standard error |
| `JUNTO_LOG_LEVEL` | `info` | `error`, `warn`, `info`, `debug` or `trace` |
| `JUNTO_LIVEKIT_API_URL` | none | Address of the LiveKit API on the private network, with `http://` (ADR 0508) |
| `JUNTO_LIVEKIT_URL` | none | Address of LiveKit for clients, with `wss://` or `ws://` |
| `JUNTO_LIVEKIT_API_KEY` | none | API key of LiveKit |
| `JUNTO_LIVEKIT_API_SECRET` | none | Secret of that key; never printed or logged |
| `JUNTO_RECORDINGS_DIR` | none | Directory of the recordings as the server sees it, an absolute path (ADR 0604) |
| `JUNTO_RECORDER_DIR` | the value of `JUNTO_RECORDINGS_DIR` | The same directory as the recorder sees it |
| `JUNTO_RECORDER_LAYOUT_URL` | none | Address at which the recorder loads the layout page, with `http://` or `https://` |
| `JUNTO_FFMPEG` | `ffmpeg` | The program that joins and reads the files of a recording (ADR 0606) |
| `JUNTO_FFPROBE` | `ffprobe` | The program that reads the packets of a file |
| `JUNTO_TRANSCRIBE_COMMAND` | none | The program of the transcription worker and its first arguments, separated by spaces. Without it the server builds no transcript (ADR 0607) |
| `JUNTO_TRANSCRIBE_SEPARATE_SPEAKERS` | `true` | Whether the voices in shared sound get speaker labels (ADR 0614) |
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

The tests of recordings use a recorder in memory that behaves as step 0 measured LiveKit Egress, and each test runs the passes of the controller, the processing and the transcription itself. The recorder in memory writes files that `ffmpeg` generates, so these tests, and `cargo test -p junto-server --lib`, need `ffmpeg` and `ffprobe` with the encoders `libopus` and `libx264` on the path. No test runs against a real recorder yet.

The tests of the hand-over to the transcription worker run a stand-in script that follows the contract of ADR 0611. One more tier runs the real worker with its development model on generated speech. It is ignored in the plain run and needs the virtual environment, the models and the test speech of `workers/transcription`:

```sh
tools/with-test-postgres.sh tools/serial.sh \
    cargo test -p junto-server --test api transcriber_real:: -- --ignored
```

`tools/with-test-livekit.sh` starts LiveKit 1.13.9 in development mode as the container `junto-dev-livekit` on the local ports 17880 to 17882 and removes it afterwards. LiveKit sends its notifications to `127.0.0.1:17890` on the machine of the tests, where one test listens; on Linux, set `JUNTO_TEST_WEBHOOK_ADDRESS` to an address of the machine that the container reaches.

## Checks

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo deny check
```

## API description

`docs/api/openapi.json` is generated from the routes, and `docs/api/events.json` from the message and event types of the event stream (ADR 0206). After a change to a route, a type or an event, write both again and commit them:

```sh
cargo run -p junto-server --bin junto-openapi
```

The test `openapi` fails while a committed file differs from the code.
