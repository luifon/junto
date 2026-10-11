# 0718. Event stream client: connection, reload of the state, and reconnection

## Status

Accepted

## Context

ADR 0107 gives each device one WebSocket connection per saved server, at the full level for the selected server and at the background level for the others. ADR 0116, ADR 0206 and ADR 0207 state what the server sends, which limits it enforces, and when it answers `resync`. ADR 0715 left one interface where the stream client attaches and stated nothing else about it. The client must keep a state that follows the server (ADR 0719), must reach a correct state after every loss of the connection, and must not reach a limit of the server by its own behaviour. The HTTP library of the client is fixed by ADR 0714: reqwest 0.12 with rustls and the certificates that the operating system trusts.

## Decision

Transport:

- The HTTP library of the API client makes the upgrade request `GET /api/v<version>/events`. A second client object of that library is used, with the settings of the first: no redirect, the same TLS library, the same trusted certificates and 10 seconds for a connection attempt. It speaks HTTP/1.1 only and has no time limit for a whole request, because a connection lasts for hours.
- The session token is in the `Authorization` header of that request and goes to the origin of the server only (ADR 0714).
- An answer with a status of 400 or above is read as the error body of ADR 0205. The client accepts the upgrade only with the status 101, the header `Upgrade: websocket` and the `Sec-WebSocket-Accept` value that belongs to its key.
- After the upgrade the WebSocket library of the server, `tokio-tungstenite`, reads and writes frames on that connection, in a task of its own. The workspace already holds both libraries, so `Cargo.lock` gets no new crate.
- A WebSocket library that opens its own TLS connection was not used: it would need a second configuration of the trusted certificates, of redirects and of the proxy settings, and the two could differ.
- The client reads messages of at most 256 KiB, which is the size of a whole batch of events on the server (ADR 0116). A larger message and a binary frame end the connection. The library answers the pings of the server.
- The transport is behind an interface with one operation: open a connection, get frames, send text. The tests replace it.

One task per server:

- The stream client of a server is one task of the client runtime (ADR 0715). It owns the state of the server and is its only writer.
- A request to the server (a snapshot, a part of the state) runs as a task of its own and reports its result. The stream task reads the connection while such a request runs, so the server never waits for the client to take a message.
- At most one request for the same thing is on its way. A request whose answer nobody waits for any more is ended, and so is a connection attempt that is no longer wanted.
- The streams start with one call, `Client::start_event_streams`. Before that call no server has a connection. A command of `junto-cli` that does one request starts none.

Levels:

- The level `selected` subscribes at `full`, and `background` at `background` (ADR 0107). With the level `off` the task holds no connection and discards the state.
- A server on which this device is in a call has at least the level `background` (ADR 0720).

Steps of a connection:

- Without a state, or with a state that must be replaced, the client reads `GET /snapshot`.
- The first message of a connection is `hello`, within 10 seconds of the upgrade. A ping does not move that time. When the epoch of the `hello` differs from the epoch of the cursor, the state must be replaced.
- The client sends `subscribe` with the level and the cursor of its state. It applies no event before the server has answered `subscribed` with that level and that cursor. The answer must arrive within 30 seconds.
- An event with an `id` at or below the position of the cursor is a repeat and is not applied. Every other event is applied, and the cursor moves to its `id`.
- An event of a type that this version does not know moves the cursor and changes nothing. A message of an unknown type is skipped (ADR 0206).
- A `checkpoint` moves the cursor. A `checkpoint` of another epoch means that the state must be replaced.

Cursor:

- The cursor is held in memory, in the state that it belongs to. It is not written to the server book.
- The client stores no state of a server on disk. After a start it has no state that a stored cursor could continue, so it reads a snapshot and subscribes with the cursor of the snapshot.
- The operation of ADR 0715 that wrote the cursor to the book is removed. The field of the book stays in the file format and is not used (ADR 0710).
- The state and its cursor belong to one account. A new session of the same account continues them. For another account the state is discarded and a snapshot is read.

Replacing the state:

- The state is replaced after: `resync` with any reason; the event `permissions_changed`; the event `call_access_ended`; another epoch in `hello` or in a `checkpoint`; a `subscribed` with another cursor than the one that was sent; a message that cannot be read or is larger than the client reads; 64 MiB of applied events since the last snapshot; a subscription at the level `full` for a state whose cursor a subscription at the level `background` has moved (ADR 0207).
- The last cause holds with and without a connection at the moment of the change: the client records the level that moved the cursor since the last snapshot.
- A message that cannot be read or is too large also ends the connection. A new connection with the same cursor would get the same message again, and the cursor of a snapshot lies after it.
- The bound of 64 MiB exists because a server can be hostile (ADR 0714): events add to the state, and a snapshot has a bounded size, so the state cannot grow without a bound.
- There is one path for all of these. From the cause on, no event is applied. The client reads a snapshot, replaces the whole state with it, and sends `subscribe` with the cursor of the snapshot, on the same connection when it is open.
- Events that arrive between the cause and the `subscribed` answer are not applied. Their effect is in the snapshot, or the server delivers them again after the cursor of the snapshot.
- The other design, to apply events while the snapshot is read and to apply the later ones again afterwards, needs a rule for every event about which copy it is applied to. One path without that rule was chosen; its cost is that the state does not change for the time of one request.
- A snapshot request that fails is sent again after the waits below.
- What the snapshot does not carry is discarded with the old state (ADR 0719).

Loss of the connection:

- The close code 4001 means an ended session. After every other close code, a failed connection, a refused upgrade and a missing `hello` or `subscribed`, the client connects again and subscribes with its cursor.
- A connection on which no frame arrived for two heartbeat intervals of the `hello` and 10 seconds more is taken as lost. The server sends a ping at each heartbeat (ADR 0116).
- The first new attempt waits 1 second, and each further one twice as long, up to 60 seconds. The actual wait is a random time between half of that and all of it, so that the clients of a server that restarted do not return at the same moment.
- The waits start again at 1 second after a connection that delivered for 30 seconds.
- A refused upgrade that names a time in `Retry-After` is not tried again before that time, cut to 60 seconds.
- The state stays during a loss. The status of the stream tells the reader that it is the last known state (ADR 0719).

Limits of the server (ADR 0116):

- One process holds one connection per server, and lets go of a connection before it opens the next one. A connection that the client lets go of ends at once and without a closing handshake, also while a message of the client waits to be taken: the server releases the place of a connection when its socket ends, and a handshake with a server that does not read would hold the place for the whole time limit.
- The client makes at most 20 connection attempts per minute for one server, also over changes between the level `off` and another level. The server accepts 30 upgrade requests of an account per minute.
- The client sends at most 20 messages per minute on one connection. `subscribe` is its only message; a further one waits. The server counts 30 frames per minute and counts the answers to its pings too.
- `too_many_connections` and `rate_limited` on the upgrade request are refusals with a wait, as above.

Ended session:

- The message `ended`, the close code 4001, and the error `unauthenticated` for the upgrade request or for a read of the stream client mean that the server no longer accepts the session.
- The stream client then holds no connection and tells the client core. The client core logs in again once with the device key (ADR 0712). A new session reaches the stream client, which continues. A ban or a removal moves the access flow to its state and the level to `off`.
- The stream client sends nothing with a session that ended. A read of a part that was on its way is sent again with the next session.
- After an ended session the client waits as after a lost connection before it sends anything with the new session: the snapshot, the reads and the upgrade request. A server that ends every session at once therefore gets the logins of a client at waits that double to 60 seconds.
- The waits belong to the server and not to the account: they stay when the next login gives another account. The level `off` ends them, because it follows an act of the user or the end of the access, after which the client logs in by no act of its own.
- The recovery notices of a device come from its login (ADR 0118, ADR 0712). `ended` with a recovery and the event `account_recovered` are told to readers as notices and change no list of recoveries.

## Consequences

- TLS, the trusted certificates and the refusal of redirects are configured in one library for requests and for the event stream.
- After a start each server costs a login, a snapshot and an upgrade request. There is no replay of the time before the start; its changes are in the snapshot.
- A permission change costs the affected client one snapshot and one `subscribe`. Readers are told that the state was replaced, not which single changes it contains.
- A client that reads an unreadable message does not receive it again: it closes the connection and continues after the cursor of a snapshot.
- For one server the client holds at most a snapshot and 64 MiB of events in its state, and at most 1024 notices of at most 256 KiB each for readers (ADR 0719). An event of this API version has a few kilobytes.
- When the login after an ended session cannot reach the server, the access flow is `logged out` and the level is `off` (ADR 0712). The stream returns with the next login.
- A second process of the same device, for example `junto-cli watch` beside the desktop client, holds a second connection of the device, which the server allows (ADR 0116). A third one closes the oldest, which then connects again after its wait.
- The stream client has no code for one operating system.
- The client is tested against a connection that the test plays, with a clock that the test moves, and against the real server through a proxy that cuts connections.
