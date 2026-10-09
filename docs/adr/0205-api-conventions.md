# 0205. API conventions: paths, identifiers, encodings and errors

## Status

Accepted

## Context

The public API is used by the client, by the setup tool and by programs that administrators write (ADR 0201), and its description is generated (ADR 0202). The records fix no path layout, no identifier format, no encoding of times and binary values, and no error format. Programs need to decide on an error without reading text that can change.

## Decision

Paths and versions:

- The server-information route is `GET /api/server`. It has no version in its path, because a client reads the supported versions from it.
- Every other route is under `/api/v<version>`. This record describes version 1.
- A route that changes state uses POST, PUT, PATCH or DELETE. PUT and DELETE on an assignment are idempotent.
- A secret value is sent in a body or in the `Authorization` header, never in a path or a query.

Identifiers and encodings:

- An identifier is a UUID of version 7 that the server generates, written with hyphens. Each kind of object has its own identifier type in the code.
- A time is written as RFC 3339 text.
- A public key, a signature and a nonce are base64url without padding (ADR 0108).
- A permission is written as its name in lower case with underscores, and a set of permissions as a list of names.
- In a PATCH body an absent field keeps its value.

Errors:

- Every response with a status of 400 or above has the body `{"error": {"code": "<code>", "message": "<text>"}}`.
- The code is stable and decides what a program does. The message is for a person and can change.
- Each code has one HTTP status. The list of codes is part of the generated OpenAPI document.
- A request for a workspace that the caller is not a member of, or for a channel that the caller cannot see, gets `not_found`. The whole response is identical to the response for an object that does not exist: the same status, the same headers and the same body. Every `not_found` response carries the same message.
- A missing permission gets `forbidden`. A limit of ADR 0301 gets its own code: `owner_only`, `owner_protected`, `own_account_target`, `exceeds_own_permissions`.
- A missing, unknown or expired session token gets `unauthenticated`.
- An internal failure gets `internal` with a fixed message. Its cause is written to the log and not sent.
- An unknown path gets `not_found`, a known path with another method gets `method_not_allowed` (405), a request that reaches the time limit gets `timeout` (503), and a request whose transaction lost eight times gets `busy` (503, ADR 0009). These responses have the same body form as every other error.
- After `busy` and `timeout` nothing was changed, and the client sends the same request again. This holds for a signed request too, because its challenge is still unused (ADR 0108).
- A ceiling of the whole server gets its own code with the status 429: `challenges_full`, `device_links_full`, `join_requests_full`.
- A body that cannot be read as the expected JSON gets `invalid_request`.

Lists:

- The audit log is read in pages from the newest entry, with `limit` (1 to 200, default 50) and `before`, the identifier of the last entry of the page before.

Generated description:

- The command `cargo run -p junto-server --bin junto-openapi` writes `docs/api/openapi.json` from the route and type definitions.
- A test compares the committed file with the generated document and fails on a difference (ADR 0202).
- Routes are registered through one list that produces both the router and the document, so a route cannot be served without being described.

## Consequences

- A client can tell the kind of an identifier only from its field, not from its text.
- Identifiers of version 7 contain their creation time and sort by it.
- A caller cannot learn from an error whether a hidden channel or a foreign workspace exists.
- A program handles an error by its code. A new code is an addition to the API and is visible as a difference in the committed document.
- The event schemas of the WebSocket stream (ADR 0202) are added with the event stream.
