# 0011. Events are written with their change and authorized when they are delivered

## Status

Accepted

## Context

A client must receive an event for every change that it may see, and no event for a change that did not happen. An event that is sent from the memory of the server after the commit of its change is lost when the server stops in between. A permission can change between the moment an event is recorded and the moment it is sent, and ADR 0301 requires that the event stream decides with the same function as every other check. The server runs as one process (ADR 0008).

## Decision

The log:

- A change writes its events into the table `events` in its own transaction, next to its audit entry (ADR 0305). An event exists when its change committed, and only then.
- The table is the only store of events. Replays read it, and live delivery reads it. The server keeps no queue of events in memory.
- An event row holds the position of its change and its ordinal in the change (ADR 0010), its time, its class (ADR 0208), its type, its payload as the client receives it, and its audience.
- The audience states as data who may receive the event: one account, the accounts with a server permission, the holders of a server role, the members of a workspace, the holders of a workspace role, the accounts with a permission in a channel, or the accounts that could see a deleted channel.
- One function records events. It refuses an audience that does not fit the type of the event.
- The function refuses a payload above 12 KiB and an audience above 48 KiB of JSON, and the change fails with it. The database refuses a payload above 16 KiB and an audience above 64 KiB of stored JSON text. The ceilings of ADR 0306 keep every event of the server below these sizes.
- An event carries no collection that can grow without a bound. Such a change is told by one event that states the rule, as `invites_of_creator_revoked`, or by a notice that makes the client load the data again (ADR 0208).

Delivery:

- Each connection reads the log after its own position, in batches of at most 64 events. A replay and live delivery are the same read.
- A batch ends at the end of a change. When the limit of rows or the budget of bytes cuts a change, the batch also reads the rest of that change. One more row, read without its audience and payload, tells whether the log holds more and whether the last change goes on, so the rest is read only for a change that the limits cut.
- The events of one change that a connection may receive are one `change` message, in the order of the change (ADR 0206). The client applies all of them before its cursor passes the change, so a connection that ends during a write leaves no cursor inside a change.
- A `change` message whose text is larger than 256 KiB, the largest message that a client reads, is not sent. The connection gets `resync` with the reason `skipped_event`, and its position moves past the change.
- One batch is one read-only serializable transaction (ADR 0009). It checks the session as every request does (ADR 0109), reads the events, and reads the memberships, roles and overrides of the account in the workspaces and channels that the batch names. From these it builds the same input for the permission function as the REST routes build.
- For each event the permission function of ADR 0301 decides whether the account may receive it now. The roles at the time when the event was recorded are not used.
- The transaction runs immediately before the messages are written to the socket. The decisions of a batch are valid for 1 second, counted from before the transaction reads anything, which is before its snapshot. An event of the batch that is not handed to the socket within that time is not sent; the next read decides it again. A batch whose read alone took longer is discarded whole; the connection reads again after 1 second and asks for half as many events. After a delivery the size doubles again, up to 64.
- The rows of one batch have a budget of 256 KiB. The read stops at the row that passes the budget.
- A part of a row that is above its largest size is not loaded.
- An event about a deleted channel reaches nobody. The event of the deletion is decided with the overrides that the channel had when it was deleted, which the event keeps, and with the current roles of the recipient.
- An event row that the server cannot send, because a part is too large or this version cannot read it, is logged. A connection whose account may receive it, or for which the server cannot tell, gets `resync` with the reason `skipped_event`, and its position moves past the change of the row; the other events of that change are not sent either. The client loads a snapshot, so the row cannot stop a connection for good. For other accounts the row is passed over.

At least once:

- The server does not record what a client has applied. The client keeps the position of the last event that it applied and sends it as its cursor (ADR 0207).
- The server can send an event again: after a new subscription with an older cursor, and after an interruption of a write. The client drops an event whose `id` is at or below its position.

Wake-ups:

- One task of the process, the watcher, reads the new rows of the log after every committed writing transaction of the process, and once per second. It reads them in pages by position and ordinal, so a page can end inside a change.
- An event that is addressed to one account wakes the connections of that account. Another event wakes the connections whose level receives its class (ADR 0107).
- A wake-up decides nothing about access. Every connection also reads the log at each heartbeat (ADR 0116), so a missed wake-up delays an event and does not lose it.

Reads of a connection:

- The event stream reads the database through a pool of its own with 7 connections. A request never uses one of them, and the event stream never uses a connection of the requests.
- A connection reads the database for a batch, for the answer to a `subscribe` and for a check of its session. Batches and answers share 4 places; checks of sessions have 2 places of their own, so slow batches do not delay them. The seventh connection is for the watcher.
- A read keeps its place until its work in the database has ended: the transaction has committed or failed and its connection is back in the pool. The place is not given up when the connection that asked no longer waits. At most 4 batches and 2 checks therefore occupy a database backend at any time, also while reads are interrupted or given up.
- PostgreSQL ends a statement and a wait for a lock of this pool after 5 seconds.
- The wait for a place and the read together have a deadline of 5 seconds for the connection. A connection that passes it stops waiting and tries again after 1 second; for a `subscribe` the connection closes with the code 1011. A read that has no place yet when its connection stops waiting does not start.
- A replacement of the connection, a stop of the server and a signal that the session can have ended end the wait at once.
- A connection that reads 5 times in a row without a delivery, because the read failed, passed its deadline or was too old to send, is closed with the code 1011. So is a connection whose session cannot be checked 5 times in a row. The client connects again with its cursor.

End of a session:

- The events of a ban, a removal, a device removal, a logout and a recovery name the account whose sessions the change can have ended.
- The watcher makes the connections of that account stop writing, reading and waiting, and check their session. A connection whose session no longer counts sends `ended` with the reason and closes with the code 4001.
- The reason is the one that the change stored for the session when it ended it (ADR 0208).
- Every read of a batch checks the session as well, and every connection checks it when it opens and at each heartbeat. A session that expires ends its connection at the next heartbeat.

Stop of the server:

- A stop between the commit of a change and the delivery of its event loses nothing. The event is in the log; the client connects again with its cursor and receives it.
- A stop signal closes every connection with the code 1001. The server waits until the task of every connection has ended, its close handshake included, for at most 5 seconds, and then aborts the tasks that are left.

Several processes:

- The first version runs one server process (ADR 0008). The order of the events, the log and the decision about each delivery are in the database and do not depend on that.
- A second process on the same database delivers the events that the first one committed, in the same order, at the latest after the interval of the watcher. The host commands (ADR 0114) use this: their events reach the connected clients within one second.
- These parts are per process and are not shared: the ceilings and the rate of opened connections (ADR 0116), the limits on attempts (ADR 0111) and the list of accounts in a call (ADR 0116). A deployment with several processes needs them in a shared store, and a notification between the processes when the one-second interval is too long. Neither is built.

Tests:

- The transactions of the event stream that no request waits for are not counted and not held by the commit gate (ADR 0009).
- The registry of connections can hold all deliveries. Only tests use this (ADR 0904).

## Consequences

- A change and its events commit together or not at all.
- A permission that is removed hides every event that the account has not been sent. An event whose decision is less than 1 second old can still be written, and a message that the socket has taken is completed.
- A database that needs more than 1 second for the read of the smallest batch delivers no events. The connections close after 5 attempts and the clients connect again.
- The memory of a batch grows with the largest change that it reads, beyond the 64 rows and 256 KiB of its limits. A change that records many events for one recipient is not sent to it and leads to a snapshot.
- The event stream needs 7 database connections next to the 16 of the requests.
- A permission that is granted shows the events about the channel that still wait in the log for that connection. A client can therefore receive an event about a channel before the notice that its permissions changed (ADR 0208).
- A subscription with an older cursor is decided by the current permissions as well. An account can read the kept part of the log (ADR 0207) for the objects that it may see now, their earlier names and their former members included.
- Each batch costs one transaction with several statements per connection. An event for many connections costs as many transactions, at most 4 at a time, so the delay of a delivery grows with the number of connections.
- A cache of the roles of a connection, valid up to the position of the last event that changed a permission, would remove most of these reads. It is not built: with it, a change of permissions that records no event would deliver events to an account that may not see them.
- The delivery reads are serializable transactions. PostgreSQL can end a concurrent writing transaction because of one; that transaction is run again (ADR 0009).
- A client that does not move its cursor receives events again after each reconnect.
- The server keeps no state per device for the stream. A device that reconnects after any time gives its cursor and gets a replay or the instruction to load a snapshot (ADR 0207).

What positions tell about a change:

- Positions count changes, not events (ADR 0010). Two events of one change that a recipient receives have positions that differ by the number of events between them that the recipient receives, which is zero. A recipient that makes a visible change before and after a change of another account counts one change in between, whatever number of events that change recorded.
- A writer therefore needs no rule for the number or the order of the events of a change. A notice that a muted or quiet account does not get, the invitation of a silent ring, the removal of the exports of other accounts, the notices to the peers of an account and the end of calls change the events of a change and not the positions that a recipient reads. The rows that only kept positions, with the audience `suppressed`, are not written any more, and the migration removed them.
- Known limit: a change that records no event takes no position. A recipient that brackets a change of another account on a server without other activity learns whether that change recorded at least one event. Each change named above records at least one event in every case: the message, the deletion, the ban or removal, or the event of the call. A writer that adds a change whose every event can be absent for a reason that a recipient may not learn records one event in every case.
