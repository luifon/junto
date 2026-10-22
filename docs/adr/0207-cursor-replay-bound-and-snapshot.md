# 0207. Cursor, replay bound and snapshot

## Status

Accepted

## Context

ADR 0107 states that each device has a cursor, and that on reconnect the server replays a bounded number of missed events or sends a fresh snapshot. It does not state what a cursor is, what the bound is, how long the server keeps events, and how a snapshot and the events after it fit together without a gap.

## Decision

Cursor:

- A cursor is the epoch of the log and a position (ADR 0010). It means: the holder has the effect of every event up to this position that it may receive.
- The device keeps its cursor. The server stores no cursor per device.
- A client moves its cursor to the `id` of each event that it applied, to the `position` of each `change` after its events (ADR 0011), and to the cursor of each `checkpoint`.

Subscription:

- After `hello` the client sends `subscribe` with a level (ADR 0107) and its cursor. Without a cursor the delivery starts after the newest event.
- A client can send `subscribe` again on the same connection, to change the level or to start from another cursor.

Replay bound:

- The server serves a cursor when all of these hold: the epoch is the epoch of the log; the position is not above the newest position; the position is not below the oldest position that the log still keeps; and at most 1000 events lie after the position that the account may receive at the level.
- For the last condition the server decides every event after the cursor as a delivery does (ADR 0011) and counts the ones that it would send. Events that the account may not receive, and events of a class that the level does not receive, do not count.
- Otherwise the server answers `resync` with the reason `unknown_cursor` or `too_far_behind`. The connection stays open and delivers nothing. The client loads a snapshot and subscribes with the cursor of the snapshot.
- A subscription ends with `resync` as well when the epoch changes (ADR 0010), when its position falls below the kept part of the log, and when the server cannot send an event that the account may receive (`skipped_event`, ADR 0011).

What `resync` means:

- After `resync`, for any reason, the client no longer knows which changes it missed. It discards every collection of that server that it holds, loads a snapshot, and subscribes with the cursor of the snapshot.
- This includes the collections that the snapshot does not carry. The client loads them again from their routes when it needs them. A client that keeps one of them over a `resync` can show an invite that was revoked or a member that was removed.
- The server sends no repair for single collections. The rule is part of the description of the `resync` message in `docs/api/events.json` (ADR 0206).

What positions tell a recipient:

- Positions are the same for every recipient (ADR 0010). A recipient learns the positions of the events that it receives, of the cursors of `subscribed` and `checkpoint`, and of a snapshot.
- From them it can tell how many positions the server used in a span of time. That number contains the changes that it did not receive and the positions of transactions that failed.
- It cannot tell which of those positions are changes, how many events a change recorded, of which type or class they are, whom they are for, or which workspace or channel they concern (ADR 0010, ADR 0011).
- A `checkpoint` is sent only when the connection has read the log to its end, and then carries the newest position. It is the same for both levels.
- The answers `unknown_cursor` and `too_far_behind` tell nothing more: the first depends on the epoch and the newest position, the second on the number of positions that the log keeps and on the events that the account may receive.

Log:

- The log keeps the newest 5,000 changes, whatever number of events each of them recorded (ADR 0010). The maintenance task (ADR 0009) stores the position of the newest change that it removes, and then removes the events up to it once per minute, in batches.
- A connection that is subscribed and whose position falls below the kept part gets `resync`.

Snapshot:

- `GET /api/v1/snapshot` returns the caller's account, the workspaces that it may see, and for each of them its roles, the caller's own roles, its categories and the channels that the caller may see.
- The route reads all of it in one read-only transaction. The cursor of the snapshot is the highest position that this transaction sees.
- A change and its events commit together (ADR 0011), and a higher position is never visible before a lower one (ADR 0010). The snapshot therefore contains the effect of every event up to its cursor, and of no later event.
- The snapshot carries: the account with its server permissions and workspaces, and for each workspace its roles, the caller's own roles, its categories and its channels, and the workspace permissions of the caller in the workspace and in each of those channels (ADR 0310).
- The snapshot also carries the calls in progress as the caller may see them (ADR 0510): the call of the caller's account, the calls of the voice channels that the caller sees, the caller's direct-message calls, the invitations into calls that wait for the caller, the invitations that the caller made and that wait, the moves between voice channels that wait and name the caller, and the caller's own requests to enter that wait.
- Loaded on demand from their routes, and not in the snapshot: the members of a workspace, the accounts, the devices of the account, the invites, the join requests, the server roles and the channel overrides.
- A client that loads one of these collections while it is subscribed applies to it the events that arrived since it sent the request, in their order, after the response. The events carry whole objects, so applying one twice changes nothing.

Checkpoint:

- At a heartbeat (ADR 0116) the server sends `checkpoint` with the newest position of the log, when the connection has read the log to its end and that position is above the last one that the client was told.
- A connection that receives few events therefore keeps a cursor near the newest position, and a reconnect does not count the events that it could not receive anyway.

Change of level:

- The background level does not deliver changes of the structure. A client that changes a server from background to full loads a snapshot and subscribes at the full level with the cursor of the snapshot.

## Consequences

- A snapshot followed by the events after its cursor gives the state of the server. A test takes snapshots during concurrent changes and compares the result with a later snapshot.
- A replay sends at most 1000 events plus the events that arrive during it.
- A client that was disconnected for longer than the log keeps, or during more than 1000 events, loads a snapshot.
- A server that records more than 5,000 changes within one minute keeps more than that bound until the next run of the maintenance task.
- The count of the replay bound reads the kept part of the log after the cursor and decides each event, 500 at a time. The cost of a `subscribe` grows with the length of that part.
- A reconnect loads a snapshot only because of events that the account would have received. An account cannot push another account over the bound of 1000 events with events that the other account may not see.
- The bound of the kept log counts changes, whoever their events are for. A recipient that finds the trimmed boundary with cursors therefore learns the number of changes after it, not the number of events of a change that went to other accounts: a ring of a muted conversation, which records no event for the muted account, moves the boundary as an ordinary ring does. A connection whose cursor predates the stored trim boundary loads a snapshot. Every route that records an event for a request of one account has a limit of requests, so one account uses a bounded number of positions per minute: message changes (ADR 0408), the changes of the own profile and of the names of the own devices (ADR 0119), and the requests of calls (ADR 0509).
- After a restore of an older backup the epoch changes (ADR 0010), every cursor is unknown, and every client loads a snapshot.

A message and its notice are one change, so the default 5,000 changes retain 5,000 ordinary messages whether or not they have notices. The number of rows is the sum of the events of the kept changes: a message with a notice has two, and a change of a call or of a moderation can have more; the request limits of the routes bound how fast one account adds changes. Replay limits count the rows eligible for the connection, excluding notices addressed to others before applying the bound; final authorization remains at delivery. The trimmed cursor boundary is the position of a change, and every row of that change is removed with it.
