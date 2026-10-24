# 0116. Limits of the event stream: connections, heartbeat and slow clients

## Status

Accepted

## Context

ADR 0107 gives each device one WebSocket connection per server. A connection is held open for hours, so a server needs to know that the client behind it still exists, how many connections an account can hold, and what happens when a client reads more slowly than events arrive. A queue per connection that grows without a bound lets one slow or hostile client use the memory of the server.

## Decision

Opening a connection:

- A device opens `GET /api/v1/events` as a WebSocket upgrade request with its session token in the `Authorization` header (ADR 0205). The session is checked as for every request (ADR 0109). Without a valid session the answer is `unauthenticated` and no connection is upgraded.
- An account opens at most 30 connections per minute. A further request gets `rate_limited`.
- A device holds at most 2 connections. A further connection of the device closes the oldest one with the close code 4002.
- An account holds at most 10 connections. A further connection is refused with the error `too_many_connections` (429).
- A connection counts for its account from the upgrade request until its task has ended and its socket is released. A connection that was told to close, a replaced one included, still counts until then.
- The two ceilings differ on purpose. The newest connection of a device is the one that its client uses, so the older one is closed. One device must not be able to close the connections of the other devices of the account, so the ceiling of the account refuses.

Heartbeat and idle time:

- The server sends a WebSocket ping every 25 seconds. The first message of the connection states this interval.
- A connection from which no frame arrived for 60 seconds is closed with the code 4004. The server checks this at each ping.
- A connection that has no subscription for 30 seconds is closed with the code 4004. The time starts when the socket has taken the `hello`, and again when it has taken a `resync` (ADR 0207), so a client always receives that message before the close.

Client messages:

- A client message has at most 4096 bytes. A larger one closes the connection with the code 1009.
- A binary frame, text that is not JSON, and a message of an unknown type close the connection with the code 4005. An unknown field is ignored.
- A client sends at most 30 frames per minute, pongs included. More close the connection with the code 4006.
- A client message is one frame. A frame that is a part of a fragmented message closes the connection with the code 4005.
- The server enforces these two rules on the bytes of the socket, where it reads the header of each frame, before the WebSocket library joins frames to messages. Each frame that a client sends is therefore counted.
- A frame that does not arrive whole is no frame. A client that sends nothing else is closed as idle.

Slow clients:

- A connection has no queue of events in memory. It holds one batch that it read from the log, of at most 64 events and 256 KiB plus one event, and the rest of its last change up to the 256 KiB of the largest message (ADR 0011), the control messages that it has not sent yet, and a write buffer of at most 1 MiB.
- The client must take each message within 10 seconds. The time starts when the socket holds the message and no earlier message waits, and it is one fixed moment: an interruption of the write, a check of the session and a new attempt do not move it. After it the server closes the connection with the code 4003 as far as the client still reads, and drops it.
- The cursor of the client is the position after the last change that it applied, and a change arrives in one message, so the disconnection loses nothing. The client connects again and subscribes with that cursor (ADR 0207).

Control messages:

- `hello`, `subscribed`, `resync` and `checkpoint` wait in the connection until the socket has taken them. A check of the session that interrupts a write does not drop them, and a message that the socket has taken is written before anything else.
- A subscription starts when the socket has taken its `subscribed`. No event of a subscription is sent before that message.

Close codes of the server:

| Code | Name | Meaning |
|---|---|---|
| 1001 | `stopping` | The server stops. |
| 1009 | `message_too_big` | A client message was larger than the limit. |
| 1011 | `failed` | The server could not read its database for the connection: for a `subscribe`, or several times in a row for a batch or a check of the session (ADR 0011). |
| 4001 | `ended` | The session no longer counts; an `ended` message came before (ADR 0011). |
| 4002 | `replaced` | A newer connection of the same device took the place. |
| 4003 | `slow` | The client did not take a message in time. |
| 4004 | `idle` | No frame within the idle time, or no subscription in time. |
| 4005 | `invalid_message` | A client message was not a known message. |
| 4006 | `too_many_messages` | The client sent more frames than the limit. |

- After 1001, 1011, 4002 and 4003 the client connects again with its cursor.

Accounts in a call:

- The server process keeps a list of the accounts that are in a call, with the place of the call: a voice channel, or the two accounts of a direct message. A connection of such an account receives every event of that call at any level (ADR 0107), when the account may receive it (ADR 0011).
- The call routes enter and remove the accounts, and every admission check replaces the list with the members that the database records (ADR 0510).

## Consequences

- The memory of one connection has a fixed upper bound that does not depend on how fast the client reads.
- A client that reads too slowly is disconnected and continues from its cursor. When it stays slow, it falls behind the replay bound and loads a snapshot (ADR 0207).
- A client must answer pings. The WebSocket libraries do this without code in the client.
- A third connection of a device ends the first one, also when the first one still worked.
- A device that reconnects faster than its old connections close reaches the ceiling of its account and is refused until they have closed. A connection that was told to close needs at most the 2 seconds of its close handshake.
- The ceilings and the rate are kept in the memory of the server process (ADR 0011). They count per process.
- The limits are values of the server configuration in the code. No environment variable changes them.
