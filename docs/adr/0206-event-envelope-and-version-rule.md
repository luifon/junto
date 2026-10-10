# 0206. Event envelope and the version rule of the event stream

## Status

Accepted

## Context

The event stream is part of the public API (ADR 0201) and other programs read it. Later build steps add events for messages, calls and recordings (ADR 0903). A client of today must keep working when a server sends an event that the client does not know, and a server must be able to add events without a new API version for each one. ADR 0202 requires a generated schema for every event.

## Decision

Frames:

- Every message is one JSON object in one WebSocket text frame. Its field `type` names the message.
- Identifiers, times and permission names are written as ADR 0205 states.

Messages of the server:

- `hello`: the first message of a connection. It carries the version of the server, the API versions it supports, the epoch of the log (ADR 0010), the heartbeat interval and the idle time (ADR 0116).
- `subscribed`: the answer to a `subscribe` that the server serves, with the level and the cursor from which events follow.
- `event`: one event, as `{"type": "event", "id": <position>, "at": <time>, "event": {"type": <event type>, ...}}`.
- `checkpoint`: a cursor up to which the connection has received everything that it may receive (ADR 0207).
- `resync`: the instruction to load a snapshot, with the reason. It answers a `subscribe` that the server does not serve, and it ends a subscription that the server cannot continue (ADR 0207).
- `ended`: the last message of a connection whose session no longer counts, with the reason, and with the recovery when a recovery was the reason (ADR 0208).

Messages of the client:

- `subscribe`: the level and the cursor (ADR 0207). It is the only client message.

Version rule:

- The stream belongs to the API version of its path, `/api/v1/events`.
- Within one API version the server adds and does not change. It can add a message type, an event type, and a field of an existing message or event.
- A field that exists keeps its name, its type, its meaning and, for a field with a fixed set of values, its set of values.
- A reader ignores a field that it does not know. It skips a message whose `type` it does not know. For an event whose type it does not know, it moves its cursor to the `id` of the event and applies nothing.
- A change that removes or changes a field, a value or a type needs a new API version (ADR 0201).
- The crate `junto-protocol` holds the types of every message and event and one function that reads a frame by these rules.

Schema:

- The command `cargo run -p junto-server --bin junto-openapi` writes `docs/api/events.json` next to the OpenAPI document. It holds a schema for every server message, for the client message, and for every event by its type, and the list of close codes (ADR 0116).
- A test compares the committed file with the generated one and fails on a difference (ADR 0202). A second test checks that every event type has a schema.
- The stream route is in the OpenAPI document with the response 101.

Later events:

- A later feature adds its events as new event types. The envelope does not change for them.
- The delivery rule already knows the classes of direct messages, mentions and call invites (ADR 0208). No event type has one of these classes yet, and the schema contains no event that the server does not send.

## Consequences

- A client of this API version works with a server that sends more event types than the client knows. It does not apply them, and it does not lose its place.
- A new value of an existing field, for example a new kind of channel, needs a new API version, because a reader with fixed types cannot read it.
- A difference between the event types of the code and the committed schema fails a test.
- A program in another language can generate its types from `docs/api/events.json`.
- The server sends no binary frames and accepts none.
