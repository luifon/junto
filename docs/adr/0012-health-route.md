# 0012. Health route

## Status

Accepted

## Context

ADR 0007 states that a health endpoint reports whether the database and LiveKit are reachable. It does not state the path, the form of the answer, or who may call it. A container runtime and a monitoring program decide on the status code and call the route without a session. LiveKit is not part of the server before the calls are built (ADR 0903).

## Decision

- The route is `GET /api/v1/health`. It needs no session.
- The server runs one statement in a read-only transaction. When the database answers within 2 seconds, the response is 200 with the body `{"database": true}`.
- Otherwise the response is 503 with the error `unavailable` in the error body of ADR 0205, and `Retry-After: 1`.
- The body states nothing else about the server.
- The feature that adds calls adds LiveKit to the body.

## Consequences

- A health check of the container runtime needs only the status code.
- A caller without a session learns whether the server reaches its database, and nothing else.
- Each call costs one transaction of the database pool. The route has no limit of its own.
- The route does not tell whether the event stream delivers. The number of connections is not reported.
