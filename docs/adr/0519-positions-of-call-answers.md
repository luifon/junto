# 0519. The position in the event log that a call answer names

## Status

Accepted

## Context

A client learns the state of a call in two ways: from the events of the stream, and from the answers of the server to a request for a place (`CallJoin`) and to `GET /api/v1/call` (`OwnCallView`). The two reach the client in either order. An answer named no position in the event log, so a client could not tell whether an answer was read before or after an event that it had received. A client that used the one that arrived later could remove a microphone block or a sound block that the server still held, or keep one that the server had removed (ADR 0503).

The snapshot has this position already: its cursor and its content are read in one read transaction (ADR 0207).

## Decision

`CallJoin` and `OwnCallView` carry `cursor`, an event cursor (ADR 0206). The cursor and everything else that the answer tells about the call are read in one read transaction, as in the snapshot: the answer holds every event up to that position and no later one.

- `GET /api/v1/call` reads the cursor in the transaction that reads the call. An answer without a call carries the cursor too.
- A route that gives a place (the join of a voice channel, the acceptance of an invitation, the join of a direct-message call, the join of a move) first commits its change. Then it reads the answer in a second, read-only transaction, and signs the token from the publishing rights that this transaction read. An event gets its position when its transaction commits (ADR 0010), so a position read inside the transaction that makes the place cannot name the events of that transaction.
- The second transaction opens the access of the caller again, checks that the caller may be at the place, checks that the seat of the first transaction exists, and reads the call with the rules for a call that hides who is inside (ADR 0517). It makes no seat and writes nothing. An answer with a cursor states that the seat of its identity exists at that position. The seat is the place of a member of the call, or, for the join of a move, the seat of the move into the call that waits (ADR 0511).
- The check that the caller may be at the place comes first, from the place that the first transaction read: a caller who may no longer be there gets the refusal also when the call has ended.
- A caller whom the second transaction refuses gets that refusal: the session has ended, or the caller may no longer be at the place. The answer of the first transaction is not given to such a caller. A seat that stays is ended by the server because no media connection uses it (ADR 0509).
- When the seat or the call has ended since the first transaction, or the server could not complete the second one (the error codes `busy`, `timeout`, `internal` and `unavailable`), the route gives the answer that the first transaction read, without a cursor. This is the answer of the route before this ADR. The place was made, and the caller was allowed to have this answer when it was read. A client takes an answer without a cursor as an answer without an order (ADR 0785).

The fields are additions to version 1 of the API (ADR 0206), and they are optional: a server from before this ADR sends none, and a client uses such an answer without an order (ADR 0785).

## Consequences

- A client puts an answer and the events of one history in one order by their positions (ADR 0785). Positions of two histories are not compared.
- Each of the four routes that give a place runs one more read transaction.
- The answer of a join can hold a change that was committed between its two transactions, for example a block. The token is signed from the same read, so the token and the blocks of one answer agree.
