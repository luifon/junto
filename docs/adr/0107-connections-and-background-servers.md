# 0107. Connections, background servers and one call at a time

## Status

Accepted

## Context

A client is connected to several independent servers (ADR 0101) and shows one of them at a time. The user must still learn about direct messages, mentions and call invites on the servers that are not shown. Receiving every event from every server costs bandwidth and processing for content the user is not looking at. A client that reconnects after an interruption must reach a correct state without receiving an unbounded list of old events.

## Decision

Connections:

- The client holds one WebSocket connection per saved server.
- The selected server sends all events.
- The other servers send direct messages, mentions and call invites only. The server does this filtering.
- Every connection also receives control events: permission changes, removals, bans, deletions and the server's version.
- A server where the client is in a call sends every event of that call, whichever server is selected.
- Each device has a cursor. On reconnect the server replays a bounded number of missed events or sends a fresh snapshot.
- The connection mode for a server is chosen when joining it. The mode "connect only when selected" is available.

Calls and devices:

- A client is in one call at a time.
- An account is in a call from one device. Joining the call from a second device moves the call to that device.
- A call continues while the user looks at another server.

## Consequences

- The server filters the events of a background connection, so that connection carries only direct messages, mentions, call invites, control events, and the events that give a system notification (ADR 0742): a join request that is created, decided or withdrawn, an addition of the person to a workspace, a call that starts, and a change of a recording. Each of these keeps the audience that it has on a full connection, and a background connection receives an addition to a workspace only for the added person, a change of a recording only for the person who started it, and the start of a call only in a channel (a direct call is told by its invitation); `events::delivery::tests::a_background_connection_is_told_only_what_concerns_its_person` checks this rule. The tests `stream_access::the_background_level_receives_the_events_of_system_notifications`, `calls_state::a_background_connection_of_an_account_in_a_call_receives_the_events_of_that_call`, `effective_permissions::a_new_workspace_tells_the_accounts_that_see_it_to_load_a_snapshot` and `recordings::a_participant_with_the_permission_starts_a_recording_with_one_job_per_audio_track` (nothing for the owner outside the call) cover them, and the window tests `event_alerts::a_workspace_addition_reaches_a_window_viewing_another_server`, `event_alerts::a_join_request_reaches_an_owner_viewing_another_server` and `live::rules::the_events_of_a_call_are_system_notifications_while_no_window_is_active` show them from a server that is not selected.
- A client learns of a permission change, a removal, a ban or a deletion on every saved server, selected or not.
- A server in the mode "connect only when selected" delivers no direct message, mention or call invite to that client while another server is selected.
- After a long disconnection the client receives a snapshot, not the full list of missed events.
- A user cannot be in two calls at once, on one server or on two.
- The server tracks which device of an account is in a call and ends the first device's participation when a second one joins.
