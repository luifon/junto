# 0012. Health route

## Status

Accepted

## Context

ADR 0007 states that a health endpoint reports whether the database and LiveKit are reachable. It does not state the path, the form of the answer, or who may call it. A container runtime and a monitoring program decide on the status code and call the route without a session. LiveKit is not part of the server before the calls are built (ADR 0903).

## Decision

- The route is `GET /api/v1/health`. It needs no session.
- The server runs one statement in a read-only transaction. When the database answers within 2 seconds, the response is 200 with the body `{"database": true, "media_server": <state>, "failing_call_rooms": <number>}`.
- `media_server` is `true` when the last admission check reached the media server, `false` when it did not, and `null` on a server without a media server (ADR 0509). The route sends no request to the media server.
- `failing_call_rooms` is the number of rooms whose admission check failed 3 times in a row and has not succeeded since (ADR 0509). It is 0 on a server without a media server.
- The body also has `storage_low`: `true` while the free disk space of the recordings is at or below the reserve, `false` above it, and `null` on a server without a recorder and when the free space cannot be read (ADR 0605).
- The status depends on the database only. A server that reaches its database and not its media server answers 200.
- Otherwise the response is 503 with the error `unavailable` in the error body of ADR 0205, and `Retry-After: 1`.
- The body states nothing else about the server.

## Consequences

- A health check of the container runtime needs only the status code.
- A caller without a session learns whether the server reaches its database and its media server, and how many rooms cannot be checked. It learns no room name.
- A health check that restarts the container on a status other than 200 does not restart the server because the media server is down. A monitoring program reads `media_server` and `failing_call_rooms` from the body.
- The state of the media server is as old as the last admission check: at most the interval of ADR 0509.
- Each call costs one transaction of the database pool. The route has no limit of its own.
- The route does not tell whether the event stream delivers. The number of connections is not reported.
