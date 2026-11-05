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
- `appearance_changed`: an empty invalidation for subscriptions opting in to `appearance` (revision 12). It moves no cursor and asks the client to reload normal permission-checked snapshot, member and profile reads (ADR 0309).
- `presence_changed`: an empty, ephemeral invalidation for an opted-in subscription; no cursor changes (ADR 0401).
- `ended`: the last message of a connection whose session no longer counts, with the reason, and with the recovery when a recovery was the reason (ADR 0208).

Messages of the client:

- `subscribe`: the level and the cursor (ADR 0207), with optional `presence` and `appearance` defaulting to false (ADRs 0401 and 0309).
- `typing_watch`: up to eight places whose transient activity the client currently displays. `typing_watching` acknowledges the accepted set; it does not replay earlier activity.
- `typing`: a place and requested lifetime in milliseconds, with zero stopping activity. The server sends `typing` with account, display name, active state and remaining lifetime. These frames have no event cursor and never change subscription checkpoints (ADR 0401).

Version rule:

Document-format support, optional channel topics and search extensions use the `capabilities` field of `GET /api/server` (ADR 0201). An absent field never enables a new request. These discovery flags do not replace the explicit document and group read opt-ins that protect older fixed-enum decoders.

- The stream belongs to the API version of its path, `/api/v1/events`.
- Within one API version the server adds and does not change. It can add a message type, an event type, and a field of an existing message or event.
- A field that exists keeps its name, its type, its meaning and, for a field with a fixed set of values, its set of values.
- A reader ignores a field that it does not know. It skips a message whose `type` it does not know. For an event of a `change` whose type it does not know, it moves its cursor to the `id` of the event and applies nothing, and it applies the other events of the change.
- A change that removes or changes a field, a value or a type needs a new API version (ADR 0201).
- A request field whose meaning changes gets a new field instead, and the old field keeps its meaning for the clients that send it, within the same API version. The position of a category or a channel is such a case (ADR 0304): `position` keeps the meaning that release 0.1.0 computes, a position in the caller's view as the caller reads positions, and `place` of `UpdateCategory` and of `to` of a channel move holds the place among the entries that the caller sees after the change. A client of release 0.1.0 then works against this server: its drags, its Up and Down of a category from one read, and its setup tool end in the order that it computed, for a caller who sees every entry of the group, as an administrator does. The limit: a caller who sees fewer entries reads places, which change after each request, so a move of a category that a client of release 0.1.0 sends as several requests can end at another place than it drew; one request, such as a drag of channels, ends where it was drawn. The window of this release sends `place`. Tests: `order_compat::up_and_down_of_a_0_1_0_window_move_a_category_by_one_place`, `order_compat::a_drag_of_a_0_1_0_window_puts_the_channel_where_it_was_dropped`, `setup_tool::a_file_orders_the_categories_and_the_root_channels_exactly`.
- Group conversation values are an explicit extension negotiated with `Junto-Group-Conversations: 1` on HTTP requests and the WebSocket upgrade, or by a new group-specific route. Without that capability, existing API v1 message and call places keep their original channel and pair value set. Group search results, notices, exports and snapshot fields are omitted, and group message and file lookups are unavailable. The server retains the capability in spawned stream delivery reads (ADR 0302).
- The messages and events have a revision within their API version (`junto_protocol::events::EVENT_REVISION`). Revision 1 is the first one with a digest; revision 2 adds `thread_root` and `thread` to `message_changed` and `notice` to `notification_changed` (ADR 0722), `watching` (ADR 0502) and `screen_shares` with the slot and the kind of each share (ADR 0510) to the participant of a call, and the events `permission_rule_set` (in place of `channel_override_set`), `channel_rules_synced`, `workspace_person_permissions_set`, `server_person_permissions_set` and `channel_history_withdrawn` (ADR 0304), and was the revision before presence. Revision 11 adds `subscribe.presence` and `presence_changed` (ADR 0401). Revision 12 adds role colour and grouping fields, public account appearance, `subscribe.appearance` and `appearance_changed` (ADR 0309). Revision 13 adds the channel topic (ADR 0215). Revision 14 adds voice channel status (ADR 0522). Revision 15 adds group conversations, membership events and conversation-keyed group call places (ADR 0302). Revision 16 adds conversation visibility (ADR 0408). Revision 17 adds workspace AFK invalidation and the optional AFK mark and destination identity of a call move (ADR 0523). Revision 18 states that the position of a category in `category_created` and `category_updated` is its place among the categories that the recipient sees (ADR 0304). Revision 19 states the same of the position of a channel in `channel_created`, `channel_updated` and `channel_restored`: its place among the channels of its group that the recipient sees (ADR 0304). Revision 20 states that a recipient who sees every entry of the group gets the stored position in these events, and every other recipient the place (ADR 0304). Each addition of a message type, an event type or a field raises it by one. `docs/api/event-revisions.json` keeps the SHA-256 of `docs/api/events.json` for each revision; the generator of the documents adds the present revision when it has no digest, and refuses an event document that changed under a revision that has a digest, so that an addition cannot go out without its revision. A test compares the committed digests with the present document.
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

Revision 3 introduced the forwarded-message reference in the typed message document (ADR 0408). Revision 9 supplies the negotiated document representation described below; an event revision warning alone is insufficient for older fixed document decoders.

Revision 4 adds `message_marked_unread`, with the place, optional thread root, backward
position and read revision. The account alone receives it under current place access.

Revision 5 identifies typed channel-reference inline nodes. They carry an ID and no copied name; current server permissions govern name discovery and navigation (ADR 0408, ADR 0740). The existing everyone mention node retains its wire identity while the interface names it @channel.

Revision 6 identifies heading and small-text blocks and spoiler inline containers. All descendants remain subject to the document and client trust bounds (ADR 0408, ADR 0747).

Revision 7 changes forwarded messages to destination-owned copies with permission-checked origin metadata (ADR 0408). Unnegotiated v1 readers receive the quoted representation described below.

Revision 8 adds transient typing and watch frames. Clients send them only after a server has advertised revision 8 or later; dropping a connection discards pending activity rather than replaying it.

Revision 9 negotiates document extensions without changing the fixed enum sets of
unnegotiated API v1. Hello advertises the server revision; HTTP requests advertise
the client's decoder through `Junto-Event-Revision`. An absent header is revision
0. A modern reader opts into the typed extensions; an older reader receives a
paragraph with visible text, or a quote for a forwarded copy with its source server
and message identifier. Mention labels use current ViewAccount access and channel names use
the reader's current ViewChannel access. Concealed spoilers become Spoiler, so a
fallback cannot disclose them in notifications or previews. The stored document
is unchanged. History pages are bounded again after projection because expanded
labels can increase their bytes.

`GET /api/server` advertises document support as `capabilities.message_format.event_revision` (ADR 0201). Before a modern write,
the client checks this revision; absent metadata defaults to zero and an older format is
refused locally before POST or PUT. A request-construction refusal is known not
sent and stays in the outbox as such. The composer also refuses unsupported
formatting when hello already identifies an older server, retaining its draft.
The server requires the decoder revision for extension writes and refuses an
older editor changing a document whose format it cannot read, preventing a
plaintext fallback from overwriting concealed content or typed references.

Revision 10 distinguishes server-verified local forward snapshots from unverified
source links. A forward now requires revision 10; revision 9 and older readers
receive the quoted legacy form, including the unverified qualification where
applicable. This prevents an older decoder from presenting an edited or remote
body with source-author attribution it cannot validate.
