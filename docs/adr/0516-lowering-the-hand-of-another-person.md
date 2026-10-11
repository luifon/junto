# 0516. Lowering the hand of another person

## Status

Accepted

## Context

A person in a call raises and lowers the own hand (ADR 0512). While meeting mode is on, the presenter removes the microphone block of a person who raised a hand, and a moderator with "mute members" can do the same. The raised hand then stays until that person lowers it. A presenter who has answered a hand, or who does not take it, had no way to lower it.

## Decision

- `DELETE /api/v1/channels/{channel_id}/call/participants/{account_id}/hand` lowers the hand of a person in the call of a voice channel.
- The person who runs meeting mode in that call can do this, and a person with "mute members" in the channel, the same two as for the removal of a microphone block (ADR 0512).
- The route is idempotent: a hand that is not raised stays as it is, and the answer is the same.
- The change is told with `call_participant_updated`, as when the person lowers the own hand. Nothing is told when no hand was raised.
- The audit log gets no entry and the channel no system line. ADR 0303 lists no action about hands, and a hand gives and takes no access.
- In a voice channel that hides who is inside, a caller outside the call gets `not_in_call` for every account (ADR 0504).
- A person who is not in the call, and a channel without a call, get `not_found`.
- The route needs a session. No access token reaches it (ADR 0210).

## Consequences

- Lowering a hand removes no microphone block and sets none.
- A person whose hand was lowered can raise it again at once.
- When meeting mode has ended, the person who ran it lowers no hand of another person without "mute members".
- A moderator outside the call lowers a hand in a channel that shows who is inside, as for a microphone block.
