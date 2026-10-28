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

- `hello`: the first message of a connection. It carries the version of the server, the API versions it supports, the epoch of the log (ADR 0010), the heartbeat interval and the idle time (ADR 0116), and the revision of the messages and events.
- `subscribed`: the answer to a `subscribe` that the server serves, with the level and the cursor from which events follow.
- `change`: the events of one change that the connection may receive, in the order of the change, as `{"type": "change", "position": <position after the change>, "events": [{"id": <position>, "at": <time>, "event": {"type": <event type>, ...}}, ...]}` (ADR 0011). The client moves its cursor to `position` after it has applied the events. The positions are those of ADR 0010. Before the first release of the API it replaced the message `event`, which carried one event, so that a change reaches a client whole.
- `checkpoint`: a cursor up to which the connection has received everything that it may receive (ADR 0207).
- `resync`: the instruction to load a snapshot, with the reason. It answers a `subscribe` that the server does not serve, and it ends a subscription that the server cannot continue (ADR 0207).
- `ended`: the last message of a connection whose session no longer counts, with the reason, and with the recovery when a recovery was the reason (ADR 0208).

Messages of the client:

- `subscribe`: the level and the cursor (ADR 0207). It is the only client message.

Version rule:

- The stream belongs to the API version of its path, `/api/v1/events`.
- Within one API version the server adds and does not change. It can add a message type, an event type, and a field of an existing message or event.
- A field that exists keeps its name, its type, its meaning and, for a field with a fixed set of values, its set of values.
- A reader ignores a field that it does not know. It skips a message whose `type` it does not know. For an event of a `change` whose type it does not know, it moves its cursor to the `id` of the event and applies nothing, and it applies the other events of the change.
- A change that removes or changes a field, a value or a type needs a new API version (ADR 0201).
- The messages and events have a revision within their API version (`junto_protocol::events::EVENT_REVISION`). Revision 1 is the first one with a digest; revision 2 adds `thread_root` and `thread` to `message_changed` and `notice` to `notification_changed` (ADR 0722), `watching` (ADR 0502) and `screen_shares` with the slot and the kind of each share (ADR 0510) to the participant of a call, and the events `permission_rule_set` (in place of `channel_override_set`), `channel_rules_synced`, `workspace_person_permissions_set`, `server_person_permissions_set` and `channel_history_withdrawn` (ADR 0304), and is the revision of the first release. Its digest was written again at each join of the branches of that release, before any release carried it. Each addition of a message type, an event type or a field raises it by one. `docs/api/event-revisions.json` keeps the SHA-256 of `docs/api/events.json` for each revision; the generator of the documents adds the present revision when it has no digest, and refuses an event document that changed under a revision that has a digest, so that an addition cannot go out without its revision. A test compares the committed digests with the present document.
- `hello` carries `event_revision` (absent from a server before the revisions: 0). A client that reads a higher revision than its build logs it once, logs the first message and the first event of each type that it does not read (at most 64 types for a connection), and the menu of the server in the window states that the server is newer than this Junto and that an update shows every change. The client applies nothing of what it does not read, as before. Reason: before the revisions a client of an older build skipped the new events without a trace, and neither the person nor a log told that changes of the server were not shown. Tests: `events::tests::server_messages_round_trip_and_carry_their_names` (a hello without the field reads as revision 0, an unknown event and message keep their type), `stream::tests::the_view_states_the_revision_of_the_events_of_the_server`, `view::book_tests` (the release of the server states the newer revision), and the server test `openapi::the_event_document_has_the_digest_of_its_revision`.
- The crate `junto-protocol` holds the types of every message and event and one function that reads a frame by these rules. It reads each event of a `change` on its own, so an event of an unknown type does not stop the reading of the others, and it states the length of each event in the frame.

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
