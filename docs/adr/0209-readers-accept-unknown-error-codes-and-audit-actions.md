# 0209. A reader accepts error codes and audit actions that it does not know

## Status

Accepted

## Context

ADR 0206 lets a server add event types within one API version, and a reader skips an event that it does not know. ADR 0205 states that a new error code is an addition to the API, and ADR 0305 states that a new audit action is a new variant. The types of `junto-protocol` read both as closed sets: a client that was built before a code or an action existed failed to read the whole response. The client and the server are upgraded at different times (ADR 0101).

The crate also derived the OpenAPI description for every type. A client compiled that derivation and its dependency without a use for it.

## Decision

Reading:

- Within one API version the server can add an error code and an audit action.
- A reader of `junto-protocol` reads an error code that it does not know as the value `unknown`. The status of the response still tells the kind of the error.
- A reader reads an audit entry with an action that it does not know as an entry with the event `unknown`. The identifier, the time, the acting account, the workspace and the channel of the entry are read as before.
- An event of the stream with an unknown type is read as before (ADR 0206).
- A reader reads an account status and a reason of `ended` that it does not know as `unknown` too (ADR 0121); a client treats such a reason as an ended session.
- Every other set of values stays closed: a new value needs a new API version (ADR 0206).

Writing:

- Writing is strict. The value `unknown` cannot be written: the encoding fails.
- The OpenAPI document lists the known codes and actions only.
- The server refuses a stored audit entry that it reads as `unknown` with an internal error. This happens only when an older server reads the database of a newer one.

Crate features:

- Secret values of the crate erase their text when they are dropped: session token, owner code, invite code, link code and call token. A copy that a caller makes is not erased with the value.
- The OpenAPI derivation of `junto-protocol` is the cargo feature `openapi`. The server enables it. A client does not, and then does not compile `utoipa`.

The existing refusal to remove the last device changes from `conflict` to the more specific `last_device` within API version 1, with the same status 409. This is an accepted error-code addition under ADR 0205 and this record, not a change to a response field or to when removal is refused (ADR 0206). A reader that predates that code reads `unknown` and still recognizes the conflict status. The refusal changes no device or session.

## Consequences

- A client of today shows an error of a later server by its status and its message, and shows a later audit entry without its details.
- A program that decides on an error code must have a branch for `unknown`.
- A test builds the crate without the feature, and the server tests build it with the feature.
- Erasing a secret on drop does not cover the copies in buffers of the HTTP library and of the JSON reader.
