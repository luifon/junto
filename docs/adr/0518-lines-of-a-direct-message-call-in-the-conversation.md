# 0518. Lines of a direct-message call in the conversation

## Status

Accepted

## Context

ADR 0501 puts the events of a call of a voice channel as system lines into the text chat of the channel. A direct-message call kept nothing: when it ended, neither account could read that it had taken place, and an account that did not answer learned of the call only while its device was connected. ADR 0515 decides that a caller cannot tell a muted conversation from a call that is not answered, and ADR 0404 decides that a muted conversation does not notify. A stored record of a call must keep both.

## Decision

The lines:

- A direct-message call writes system lines into the direct-message conversation of its two accounts (ADR 0408). When the two accounts have no conversation, the first line creates it, with `conversation_opened`.
- `direct_call_started`: the first ring of the call was made (ADR 0515). The actor is the caller, the subject the other account. A later ring of the same call, which an account that waits alone makes with a new place, writes no second line of the start; its end writes its own line.
- `direct_call_not_answered`: the invitation ended and the other account had taken no place in the call, because the 60 seconds of the invitation passed or the call ended before. The line is also written at the end of a call in which the other account took a place and was never inside (ADR 0517), because its device did not connect to the media server, when no ring of the call has ended with a line. An account that was inside the call has no missed call, also when the caller was never inside. The actor is the caller, the subject the other account.
- `direct_call_declined`: the other account declined the ring. The actor is the account that declined, the subject the caller.
- `direct_call_ended`: the call ended after both accounts were inside it (ADR 0517). The line states `duration_seconds`, counted from the check that first saw both accounts in the room to the moment from which one of them was alone until the end; for a member whose connection was lost, that moment is the loss. A member whose connection is lost is not in the room for this rule. A call in which the two accounts were never in the room together has no such line.
- Each line is written in the transaction of the change that it states. A line is public in the conversation: both accounts read it while they may read the conversation (ADR 0408), and nobody else does. Nobody edits or deletes a line. The retention of direct messages removes lines as it removes messages (ADR 0409). A line is no search result.
- No line is written while the two accounts cannot reach each other (ADR 0307) or one of them is not active. The conversation is closed to both then, and a line would wait in it unread for the day on which the contact rule holds again.

A call that was not answered:

- The line `direct_call_not_answered` counts as one unread message for the account that was called, and for nobody else. No other system line counts as unread.
- That account gets a notice of the kind `missed_call`, whose message is the line. The policy of the conversation decides it as for a direct message: an account that muted the conversation gets no notice (ADR 0404). An account in its quiet time is no receiver of the event of the notice, as for every notice (ADR 0409), also when it reads the log after the quiet time; the notice is stored and is in the list and in the snapshot.
- Reading the conversation up to the line marks the notice read, as for every notice.

The caller cannot tell a mute:

- The lines, their order and their times are the same for an account that muted the conversation, for an account in its quiet time and for an account that did not answer. A ring for such an account is the silent invitation of ADR 0515; it ends with the same line as every ring.
- What differs is stored for the other account only: its notice, its unread sign and its read position. The caller reads none of them.
- The notice of the other account records one event that only that account receives. The number of positions that the log of events used is therefore one higher for an account that is notified, as it is one higher for an invitation that is not silent. ADR 0207 states what a recipient can read from positions: on a server without other activity, a caller who counts positions can tell the two cases. No rule of this record hides that number.
- An account that muted the conversation receives the lines as it receives a message of a muted conversation: the event `message_changed`, without a notice and without an unread sign. This narrows the sentence of ADR 0515 that the muted account learns nothing of the call: it is not rung and gets no invitation, and it reads the line in the conversation.

Storage:

- The call stores the account whose entry made the first ring, the time from which both accounts were in the room, the time from which one of them is alone again, and whether the call owes no missed call at its end, because a ring ended with a line or the other account was inside. They are read when the call ends, and the row of the call is deleted with it.
- A message stores the account for which a system line counts as unread. The unread count of a place reads those lines through an index of the place, that account and the position, at most 100 of them.

## Consequences

- A person who was offline during a call reads in the conversation that the other person called, and has a notice and an unread sign for it.
- A conversation between two accounts can exist that neither opened by hand: a call creates it.
- A caller who hangs up after two seconds leaves the line `direct_call_not_answered` and a notice. The limits of rings bound their number (ADR 0515).
- The other account that enters a call after the line `direct_call_not_answered` was written gives the call a duration, and the conversation then has both lines.
- A device that accepts a ring and does not reach the media server leaves a missed call for its account and no duration.
- A person who does not want to read lines of calls from an account has no setting for it. An operator bans an account that abuses calls (ADR 0113).
- The lines of a call are content of the conversation: they are in an export of it, with the duration (ADR 0411).
