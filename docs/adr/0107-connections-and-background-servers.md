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

- The server filters the events of a background connection, so that connection carries only direct messages, mentions, call invites and control events.
- A client learns of a permission change, a removal, a ban or a deletion on every saved server, selected or not.
- A server in the mode "connect only when selected" delivers no direct message, mention or call invite to that client while another server is selected.
- After a long disconnection the client receives a snapshot, not the full list of missed events.
- A user cannot be in two calls at once, on one server or on two.
- The server tracks which device of an account is in a call and ends the first device's participation when a second one joins.
