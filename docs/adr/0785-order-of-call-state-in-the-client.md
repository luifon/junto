# 0785. Order of call state in the client

## Status

Accepted

## Context

Call state reaches the client through snapshots, events, join answers, token renewals and own-call reads. Arrival order does not establish the order of the state they describe. Replaying an included event over a newer HTTP answer can restore an obsolete participant, block or meeting state. A microphone block that a token renewal returned was removed by an older state of the client, a read of the own call rolled blocks back, and a meeting grant was lost when its notice was discarded.

## Decision

Every writer compares the cursor of its observation (ADR 0519) with the last accepted cursor for that call in the same epoch. An older observation cannot replace a newer one. A read of the own call replays only later events; an included event delivered after the answer cannot change the call fields already described by that answer. Effects on other collections and speaking marks, which the call answer does not include, continue to follow their own event history.

A call read may be ahead of the stream without advancing the stream cursor. Membership and pending moves still use the stream cursor; call participants, blocks and meeting state use the call cursor (`ServerModel::call_cursor`). A later call read does not turn older membership into a current observation.

The media session applies the same rule to join answers, renewal answers and the live model. Admission and participant actions require the server to list the current seat in state at least as new as its join answer; until then the microphone and the sound of the call stay closed on the device. Restrictive controls are applied before a renewed media connection can send or play sound. Another epoch is reconciled as another history, not compared by its numeric position: the session asks the client core for a snapshot that is requested after the question (`LiveServer::refresh`) and continues from it.

The answer to a request for a new token (a renewal answer) is a writer like the others, and it carries the state of the person at its cursor. The rule for it:

- An answer with a cursor of the same epoch, for the seat that the session holds, changes no admission. When its position is at or after the last accepted one, its blocks are applied at once, and the state and the events of the stream up to its position do not change the blocks. When its position is before the last accepted one, it changes nothing but the token.
- An answer for another seat, an answer of another epoch, an answer without a cursor, and an answer that does not tell the blocks (ADR 0517) end the admission: the microphone and the sound stay closed until a state lists the seat. The session uses the seat of the answer in each of these cases, also for another epoch: a seat that the server gave and that no device uses would stay.
- The fields of the renewal answer that this rule depends on, each read in the transaction that reads the cursor (ADR 0519): the microphone block of the person with its reason, which is the block of a moderator or the block of meeting mode (`microphone_block`); the meeting of the call with its presenter (`call.meeting`), from which the session derives a grant when the block that it held is absent; the sound block (`sound_block`); and that the seat of `identity` exists at the cursor, which the server checks before it gives an answer with a cursor.
- An answer for a call that hides who is inside tells none of these until the server has seen the device in the room (ADR 0517). A seat that the state of the client listed was seen, so an answer for an admitted seat tells them.
- An answer of a join or of a renewal with a cursor states that the person is inside with its seat at that cursor. It is the membership of the person at that position: while the stream is live and its state is from before that position, the place is held, and a media connection that fails is followed by a request for a token. A state at or after that position is the membership in its place. An event at or before that position does not end the place; an event after it that ends the place ends it, also when the answer arrives after the event. While the stream is interrupted, an answer decides nothing (ADR 0734). The floor for membership and moves is the position of that answer, not the position of a later read of the call.

A device that has its account and no session, while the client core logs in again (ADR 0787), keeps its call as while the event stream is interrupted (ADR 0734): the admission and the blocks stay as the last state told them.

Notices that require an action by the person remain observable even when a newer model was published first. In particular, a meeting grant that requires the person to reopen the microphone cannot be discarded by refreshing the model (ADR 0739). The session reads the meeting of its call from the events in their order, and a grant closes the person's microphone setting once: its local mute effect remains until an explicit user action, including when meeting mode ends afterwards, and a grant that arrives after the person has opened the microphone again for the same position does not close it again. A state that shows the end of a block tells the same grant as the event of that end. The session holds the position of the last state that showed the block: when the person was told a grant after that position, or opened the microphone after it, a later state without the block does not close the microphone again.

An answer without a cursor comes from a server from before ADR 0519. The client uses it as before that ADR: the client core applies again every event that arrived during a read of the own call, and the media session takes the answer as read at the position of the state that the client holds when the answer arrives. Such an answer has no order with the events.

## Consequences

A delayed model cannot erase a returned block, and a delayed answer cannot erase a newer block. Deterministic tests deliver answers and events in both orders and check capture, playout, admission and the local setting after a meeting grant: `calls::ordering_tests` in `crates/media`, `stream::tests::refresh` in `crates/client-core`. Snapshot replacement remains recovery for unavailable history, rather than the routine way to order token renewals.

A call has no sound on a device while the state of the client does not list its seat, which includes the time in which the event stream of the server is interrupted at the start of a call, and the time after a renewal answer for another seat.

A renewal for the same seat does not interrupt the microphone and the sound beyond the time of the request: the stream of a device moves its cursor without an event of its own only at a checkpoint, 25 seconds apart, and the session does not wait for it.

The speaking marks of a call follow the events also when an event is not applied to the call because an answer already holds it: the mark of a person who left is removed.
