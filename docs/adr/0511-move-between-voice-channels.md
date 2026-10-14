# 0511. The move of a person between voice channels

## Status

Accepted

## Context

ADR 0504 decides that a person with "move members" moves another person without a prompt, that the server performs the move, that the client joins the destination first and leaves the source only after the join is confirmed, and that nothing changes when the join fails. The move function of the media server is not available in the self-hosted LiveKit. An account is in one call at a time (ADR 0510), and during a move the client is connected to two rooms.

## Decision

Request:

- A move names a person in the call of one voice channel and another voice channel as its destination.
- The person who moves needs "move members" in the source and in the destination.
- The moved person needs access to the destination. A move passes the lock and the user limit of the destination, and no permission (ADR 0504).
- The server stores the move with a new identity for the destination and tells the moved account and the account that moved with `call_move_requested`. The call of the destination is created when it has none.
- One move per account waits at a time.

While the move waits:

- The moved account is still a member of the source and nothing else has changed. Others see the person in the source only.
- The identity of the move is not yet allowed in the room of the destination. Only a completion allows it. The move takes a place for the user limit of the destination.
- Only the session that is in the call of the source gets the join token of the destination, from `POST /api/v1/call/moves/{move_id}/join`, and only while the move has not expired and the account may be at the destination. The device joins the destination and stays in the source.

Completion:

- The admission check completes the move when the media server lists the identity of the move in the room of the destination (ADR 0509).
- Before that, in the same transaction, the check decides in this order: the move has not expired; the account is still a member of the source; the account may be at the destination; the account that moved still has "move members" in the source and in the destination. When one of them fails, the move ends without effect and the identity is removed from the room of the destination.
- A join token of a move is usable for longer than the 30 seconds of the move (ADR 0508). A device that connects with it after the 30 seconds is removed, and the move is not completed.
- A move that ended on the device starts no connection to the destination: the device does not wait for a token that it asked for, and does not use one that arrives afterwards. A move ends on the device with its outcome, at its time limit, when another move replaces it, and when the call ends. A connection that is being made at that moment is closed as soon as it is made, and the end of the call waits for that, for at most 20 seconds.
- In one transaction the account stops being a member of the source and becomes a member of the destination, with its own microphone and sound settings. The block of meeting mode of the destination applies.
- The moved account and the account that moved get `call_move_resolved` with the outcome `completed`. The audit log gets two entries by the account that moved: `call_member_moved_out` about the source channel, which does not name the destination, and `call_member_moved_in` about the destination channel, which does not name the source. Each entry is shown by the rule of its own channel (ADR 0305), so a reader who sees only the source does not learn that the destination exists.
- The client leaves the source when it receives the outcome. The identity of the source is no longer allowed, and the admission check removes it.

Failure:

- A move ends without effect when the destination was not joined in 30 seconds (`timed_out`), when the session that holds the place in the source reports that it could not join (`refused`), or when the person leaves the source, the session of the person ends, the person loses the access to the destination, or the account that moved loses "move members" before the completion (`abandoned`).
- A stored move whose 30 seconds passed while the server was stopped or the media server was unreachable is discarded with `timed_out` at the first check, also when the media server lists its identity.
- Only the session that holds the place in the source reports the failure. Another session of the account gets `not_in_call`, so a second device cannot end a move that the first one performs.
- Both accounts get `call_move_resolved` with the identifier of the move and that outcome, and no channel, each while it may see both channels. A person who moved themselves gets it once. An account that can no longer see one of them gets `call_access_ended` and no outcome (ADR 0510). The person stays a member of the source.
- The admission check ends a move that waits as `abandoned` when the moved account or the account that moved may no longer see the destination.
- The identity of the move is no longer allowed. A call of the destination that the move created ends when it has no member.

## Consequences

- A client is connected to two rooms for the time between its join of the destination and the outcome: at most one interval of the admission check after the join.
- A move into a full voice channel makes it hold more people than its limit (ADR 0504). A person who joins at the same moment either takes the last place before the move is stored, or finds the place taken by the move.
- A move that fails leaves no trace in the audit log.
- A reader of the audit log who sees both channels finds two entries with the same time, the same account and the same actor, and can join them. A reader who sees one channel sees that a person was moved out or in, and not where from or where to.
- Blocks of the source do not follow the person: a block belongs to a call (ADR 0510).
- A moved person who is in a call from another device than expected, or who closed the client, makes the move time out.

Role order and its action-specific exemptions are decided in ADR 0309.
