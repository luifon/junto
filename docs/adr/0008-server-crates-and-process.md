# 0008. Server crates, configuration and process behaviour

## Status

Accepted

## Context

The server is the first unit of product code (ADR 0903). The permission rule must be one function that the client can also call (ADR 0301), and the API types are shared with the client (ADR 0201, ADR 0202). The server runs in a container (ADR 0003), where settings arrive as environment variables and output goes to the two standard streams. The first start prints a code that gives ownership of the server (ADR 0201), so that code must not reach a log collector as a log record. Tests need to control the time that expiry rules use.

## Decision

Crates:

- The repository root is one Cargo workspace with the members `crates/*`. The directory `spikes/` is excluded.
- `junto-authz` holds the permission model and the evaluation function. It performs no input or output and depends on no other project crate.
- `junto-protocol` holds the request, response and event types, the API version and the error codes. It depends on `junto-authz` for the permission names.
- `junto-server` holds the server binary. It depends on both.
- `junto-setup` holds the setup tool (ADR 0212). It depends on `junto-protocol` and not on the server.
- The workspace forbids `unsafe` code and denies `unwrap` and `expect` outside tests. The exceptions are named modules of the client crates (ADR 0711, ADR 0733) and two modules of the server on Windows: the one that reads the free space of a directory (ADR 0605) and the one that waits for the end of the process that started the server (ADR 0915).

Configuration:

- The server reads its settings from environment variables with the prefix `JUNTO_`: `DATABASE_URL` (required), `LISTEN_ADDRESS` (default `127.0.0.1:8080`), `SERVER_NAME` (default `Junto`), `TRUSTED_PROXIES` (a comma-separated list of IP addresses, default empty), `LOG_FORMAT` (`json` or `text`, default `json`), `LOG_LEVEL` (default `info`) and `STOP_WITH_PROCESS` (the number of a process, default none).
- `SERVER_NAME` is used at the first start only. The name is stored in the database afterwards.
- An invalid value stops the start with a message that names the variable.

Process:

- The HTTP layer is `axum` on `tokio`. The database layer is `sqlx` with queries that are checked at run time, so a build needs no database.
- `sqlx` is built without a TLS feature, because PostgreSQL is reachable only on an internal network (ADR 0003).
- The migrations are embedded in the binary and applied at each start under the lock that `sqlx` takes (ADR 0005). There are no down migrations.
- Every transaction is serializable and is retried a bounded number of times, and PostgreSQL enforces time limits on every connection (ADR 0009).
- A task of the server process removes expired rows once per minute (ADR 0009).
- Log records are structured and go to standard error. Each request produces one record with the method, the path, the status and the duration. Request bodies and headers are not logged.
- The owner code goes to standard output as plain text and is never a log record.
- A notice with a one-time code, at the start and from `issue-owner-code`, is one write to standard output, so that no other write of the process to standard output is between its lines; `println!` writes a text with several lines in parts. One write does not give one read: a reader of a pipe can receive the notice in several parts, and a buffer smaller than the notice always does, so a program that reads the code reads until the end of the notice. Docker reads the standard output and the standard error of a container separately and stores each line as its own entry, and `docker compose logs` writes the entries of both into one stream: in that stream a record of standard error can be between two lines of the notice, also after one write. A program that reads the code therefore reads standard output alone (see Consequences). Test: `first_start::the_binary_writes_the_notice_of_the_owner_code_in_one_write` and `the_host_command_writes_the_notice_of_the_owner_code_in_one_write` make the standard output of the binary a datagram socket, which receives each write as one datagram.
- SIGINT and SIGTERM stop the listener. Open requests are finished before the process exits.
- With `STOP_WITH_PROCESS`, the end of the process with that number, which must be the process that started the server, stops the listener as SIGTERM does, also when that process was killed (ADR 0915). The deployments do not set it.
- Every request has a time limit of 30 seconds. A request that reaches it gets the error `timeout` (ADR 0205) and its transaction is rolled back.
- The binary serves the API when it is started without arguments. With the argument `assign-owner` or `issue-owner-code` it runs a host command and exits (ADR 0114).
- Every rule that depends on the time reads it from one clock object. Time values are passed to SQL as parameters; SQL statements do not call `now()`.

## Consequences

- The client can depend on `junto-authz` and `junto-protocol` without the server's dependencies.
- An error in a SQL statement is found by a test that runs it, not by the compiler. The integration tests run every statement against PostgreSQL (ADR 0904).
- A deployment that puts PostgreSQL on another network needs a TLS feature added to the build.
- An operator reads the owner code from the standard output of the container. A program that reads the code reads the standard output of the container alone, with `docker logs`, which writes the two streams of the container to its own two streams. This also reads the code of an earlier release, which writes the notice in two writes; the upgrade test of the deployment runs such a release.
- A test replaces the clock and moves the time forward without waiting.
- The server process keeps the limits on attempts in memory (ADR 0111), so a deployment runs one server process. A host command can run next to it.
