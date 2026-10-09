# junto-server

The Junto server: accounts, device keys, invites, roles, channels, the audit log and the public API. The design is in [`docs/adr/`](../../docs/adr/README.md); ADR 0008 describes this crate.

## Run it locally

The server needs PostgreSQL; it is tested with PostgreSQL 17. This starts one in Docker and the server next to it:

```sh
docker run -d --name junto-local-postgres -e POSTGRES_HOST_AUTH_METHOD=trust -p 127.0.0.1:5432:5432 postgres:17-alpine
JUNTO_DATABASE_URL=postgres://postgres@127.0.0.1:5432/postgres cargo run -p junto-server
```

The first start applies the migrations, creates the default workspace with one text channel and one voice channel, and prints a one-time owner code on standard output. The first device that presents the code becomes the server owner. Each start without an owner prints a new code.

`GET http://127.0.0.1:8080/api/server` returns the name of the server and the API versions it supports.

## Host commands

Two commands change the database directly and are not part of the API (ADR 0114). Both read `JUNTO_DATABASE_URL` and can run while the server runs.

```sh
junto-server assign-owner <account-id>   # make an active account the server owner
junto-server issue-owner-code            # print a one-time code that returns the owner to their account
```

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

Each test creates its own database. Without the script the integration tests fail with a message that names it. Every transaction of the server is serializable and retried (ADR 0009); the tests of races hold one request before its commit to fix the order. PostgreSQL ends a statement after 10 seconds and a wait for a lock after 5 seconds; the request then gets the error `timeout`. The tests that need no database run alone with:

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

`docs/api/openapi.json` is generated from the routes. After a change to a route or a type, write it again and commit it:

```sh
cargo run -p junto-server --bin junto-openapi
```

The test `openapi` fails while the committed file differs from the routes.
