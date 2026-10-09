# junto-server

The Junto server: accounts, device keys, invites, roles, channels, the audit log, the event stream and the public API. The design is in [`docs/adr/`](../../docs/adr/README.md); ADR 0008 describes this crate.

## Run it locally

The server needs PostgreSQL; it is tested with PostgreSQL 17. This starts one in Docker and the server next to it:

```sh
docker run -d --name junto-local-postgres -e POSTGRES_HOST_AUTH_METHOD=trust -p 127.0.0.1:5432:5432 postgres:17-alpine
JUNTO_DATABASE_URL=postgres://postgres@127.0.0.1:5432/postgres cargo run -p junto-server
```

The first start applies the migrations, creates the default workspace with one text channel and one voice channel, and prints a one-time owner code on standard output. The first device that presents the code becomes the server owner. Each start without an owner prints a new code.

`GET http://127.0.0.1:8080/api/server` returns the name of the server and the API versions it supports. `GET http://127.0.0.1:8080/api/v1/health` answers 200 while the server reaches its database and 503 otherwise (ADR 0012).

## Event stream

A logged-in device receives the changes of the server over one WebSocket connection (ADR 0107).

1. `GET /api/v1/snapshot` returns the account, its workspaces, and the roles, categories and channels that it may see, with a cursor.
2. The device opens `GET /api/v1/events` as a WebSocket with its session token in the `Authorization` header. The server sends `hello`.
3. The device sends `{"type": "subscribe", "level": "full", "cursor": <cursor of the snapshot>}`. The level `background` delivers control events only until direct messages, mentions and call invites exist.
4. The server sends each event that the account may receive as `{"type": "event", "id": <position>, "at": <time>, "event": {...}}`, in the order of the positions. The device keeps the `id` of the last event that it applied; an event with an `id` at or below it is a repeat.
5. After a disconnection the device subscribes with its cursor. The server replays up to 1000 events that the account may receive. When more wait, or the cursor is not of this log, the server answers `resync`. The device then discards everything that it holds of this server, also what the snapshot does not carry, such as invites and members, and starts again at step 1 (ADR 0207).

Events are written in the transaction of the change that they describe and get their position at the commit (ADR 0010). Each connection reads them from the database and decides with the permission function, immediately before it sends them, whether the account may receive them; a decision is valid for one second (ADR 0011). A client message is one unfragmented frame of at most 4096 bytes. A removal, a ban, a logout and a device removal close the affected connections with the message `ended`. The events and their recipients are in ADR 0208, the limits of a connection in ADR 0116.

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
| `JUNTO_SERVER_NAME` | `Junto` | Name of the server and of its default workspace; read at the first start only |
| `JUNTO_TRUSTED_PROXIES` | empty | Comma-separated IP addresses of the entry hosts whose `X-Forwarded-For` header the server trusts |
| `JUNTO_LOG_FORMAT` | `json` | `json` or `text`; log records go to standard error |
| `JUNTO_LOG_LEVEL` | `info` | `error`, `warn`, `info`, `debug` or `trace` |

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
