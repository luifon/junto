# junto-server

The Junto server: accounts, device keys, profiles, invites, roles, channels, the audit log, the event stream, the server side of calls and the public API. The design is in [`docs/adr/`](../../docs/adr/README.md); ADR 0008 describes this crate.

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

## Host commands

Three commands change the database directly and are not part of the API (ADR 0114, ADR 0010). Each reads `JUNTO_DATABASE_URL` and can run while the server runs.

```sh
junto-server assign-owner <account-id>   # make an active account the server owner
junto-server issue-owner-code            # print a one-time code that returns the owner to their account
junto-server new-event-epoch             # make every client load a snapshot
```

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

The four LiveKit settings are given together or not at all.

## Tests

The integration tests need Docker. The script starts a temporary PostgreSQL, runs the command and removes the container:

```sh
tools/with-test-postgres.sh cargo test --workspace
```

Each test creates its own database. Without the script the integration tests fail with a message that names it. The tests of the event stream open real WebSocket connections to the server; some of them hold all deliveries to fix what waits in the log (ADR 0011). Every transaction of the server is serializable and retried (ADR 0009); the tests of races hold one request before its commit to fix the order. PostgreSQL ends a statement after 10 seconds and a wait for a lock after 5 seconds; the request then gets the error `timeout`. The tests that need no database run alone with:

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
