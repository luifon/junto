# 0515. Rings of direct-message calls: limit, pause after a decline, and muted conversations

## Status

Accepted

## Context

A direct-message call invites the other account when the first account enters (ADR 0514). The invitation has the class of call invites and reaches every connection level (ADR 0107), so it rings on a device that shows another server. An account that was declined could leave the call, enter it again and ring again. The only limit was the limit of call-changing requests of one session (ADR 0510), and a new session started a new count. ADR 0404 decides that a muted conversation does not notify; the mute of a direct-message conversation was not stored. A block list is not in the scope of the product (`docs/scope.md`).

## Decision

Rings:

- A ring is the invitation that a direct-message call makes for the other account of the direct message.
- A ring is made when an account takes a new place in a direct-message call while the other account is not in the call and has no invitation that waits. A request for a new token of the same place makes no ring.
- One account rings another account at most 3 times in 10 minutes. A ring that the other account accepted by entering the call is not counted.
- After the other account declined a ring, the caller cannot ring that account for 5 minutes.
- Both rules count the rings of one account for one other account. The rings of the second account for the first are counted apart.
- The rings are rows of the database. The rules hold for every session and every device of the caller, and after a restart of the server.
- A request to enter that would make a ring against these rules gets `rate_limited` (429) with `Retry-After`. The caller does not enter the call.
- An invitation into a voice channel is not a ring. Its limits are in ADR 0514.

Muted conversations:

- An account mutes its direct-message conversation with another account with `PUT /api/v1/direct-messages/{account_id}/mute`. The body states the time at which the mute ends, or nothing for a mute that lasts until it is turned off. `DELETE` on the same path turns the mute off.
- The mute is stored on the server for the account and the conversation. The snapshot carries the mutes of the account, and a change reaches the devices of the account as `direct_mute_set` or `direct_mute_removed`.
- While an account has muted a conversation, or is in its quiet time (ADR 0404), a call of the other account makes a silent invitation. The muted account learns nothing of it: no event, nothing in the snapshot, and `not_found` from the routes that accept and decline. The lines that the call writes into the conversation are read by both accounts (ADR 0518).
- For the caller a silent invitation is an invitation: the caller gets `call_invited`, the invitation in the snapshot, `call_invitation_resolved` with `expired` after 60 seconds or with `cancelled` when the call ends, and the ring is counted. The first version of this record made no invitation and counted no ring, so the caller saw other events, and a fourth call within 10 minutes was accepted only for a muted conversation.
- The caller enters the call as without the mute and is not told about it. A silent invitation reserves the same event-log positions as an ordinary invitation at creation and resolution. The suppressed recipient event has an internal audience that can never receive it, including after unmute and at replay. A missed-call notification always reserves one position, even when a mute prevents its row. This closes the position-count distinction without exposing a silent invitation.
- An account mutes a conversation with an account whose profile it may see (ADR 0119), or with an account that may call it by the contact rule (ADR 0307). The second case exists because an administrator without a common workspace may call an account that cannot see the profile of the administrator, and that account must be able to mute the calls. Every other account gets `not_found`, as an account that does not exist.
- There is no block list.

Cleaning:

- Maintenance deletes the rows of rings that no rule counts any more, and the mutes whose time has passed.

## Consequences

- A person who does not answer is rung at most 18 times per hour by one account. A person who declines is rung at most 12 times per hour by that account. The person ends it with a mute.
- A caller cannot tell a muted conversation from a call that is not answered.
- A muted account with a full connection still receives `call_started` for the direct-message call and can enter it. It gets no invitation, so its devices do not ring. When it enters, the caller gets `call_invitation_resolved` with `accepted`, as for every ring.
- A caller who rings a muted account 3 times in 10 minutes is refused at the fourth call, as for an account that does not answer.
- The limit counts rings and not calls. Entering a call that the other account is in, or that the other account was invited to within the last 60 seconds, makes no ring. Two people who call each other and answer are not limited.
- A caller who waits alone in a direct-message call for longer than the 60 seconds of the invitation rings again only by leaving the call and entering it again.
- A person who is rung against their wish by several accounts mutes each conversation. An operator bans an account that abuses calls (ADR 0113).
- The text unit uses the same mute for the notifications of direct messages (ADR 0404).

Message creation reserves one notification event even when no account is notified. This applies to channels as well as direct conversations: a channel with few members must not expose one member’s mute through event positions. Empty receiver lists deliver that event to nobody. Call rings, expiry and early cancellation preserve the order and positions of the caller’s events in muted and quiet cases as well as the ordinary case.

A message event precedes its notice event. The pair counts as one retained unit (ADR 0207), including for edits and missed-call lines, and both rows are kept or removed together. This preserves the message replay horizon while keeping equal global positions in notified, muted and quiet cases. An empty notice is suppressed; a notice with recipients wakes only their eligible connections through the indexed receiver rows. Delivery excludes notices for other accounts before counting the replay limit or filling a page. While retained, starting at a message position therefore has the same result as starting after its notice for a reader who receives no notice. After trimming the pair, the ordinary global cursor boundary applies equally in all modes.
