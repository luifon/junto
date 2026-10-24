# 0521. The state of a call states a grant of meeting mode

## Status

Accepted

## Context

While meeting mode is on, the presenter, or a moderator with "mute members", removes the microphone block of a person: a grant (ADR 0512). The client of that person closes the microphone setting once at a grant, so that the person opens the microphone with an action of their own (ADR 0785). The client learns of a grant from the event of the change of the participant. A state of the call shows only the result: the person without a block. When meeting mode ended after the grant, a state shows no block and no meeting, the same as a state after the end of meeting mode without a grant. A device that did not read the event of the grant, after missed events or a snapshot, cannot tell the two apart.

Two forms were possible: the position of the event of the last grant, or a mark that the next open microphone of the person clears. An event gets its position when its transaction commits (ADR 0010), so the transaction of the grant cannot store the position of its own event, and the log of events is trimmed, so a stored position can name an event that no longer exists.

## Decision

- Each seat holds a mark, `meeting_grant` in `call_members`.
- The removal of a microphone block of a person through `DELETE .../participants/{account_id}/microphone-block` sets the mark when the call has meeting mode and the person is not its presenter. The reason of the block does not matter, as for the client (ADR 0785).
- A new microphone block of the person clears the mark: a block of a moderator, and the block of the start of meeting mode. A trigger on `call_microphone_blocks` does it for every insert.
- A report of the own setting that changes it from muted to open clears the mark (`PUT /api/v1/call/state`). A report that does not change the setting does not clear it: a device that did not read the grant and reports the open setting that it had before does not end the mark.
- The end of meeting mode does not clear the mark. A seat that ends takes its mark with it; a new seat has none. A person who moves the call to another device during the meeting gets a new seat: it gets the block of the meeting on entry (ADR 0512) and no mark, and the person asks again.
- Only the person reads the mark: `OwnCallState.meeting_grant` in `GET /api/v1/call` and `CallJoin.meeting_grant` in the answer to a request for a place or a token. A participant of a call has no such field. A person who sees a call later would otherwise learn of a grant of another person in a meeting that the person did not see.
- An account that is not inside a call that hides who is inside gets no mark (the field is absent), as it gets no meeting and no block: a grant tells that meeting mode was on (ADR 0517).
- The fields are optional. A server from before this ADR sends none.

## Consequences

- A device that reads a state with `meeting_grant: true`, and that has not closed the microphone setting for a grant after the time from which it holds the state, closes it once. The person then opens the microphone, and the report of that clears the mark.
- The mark does not say which grant it is. After a grant, a new block and a second grant it states only that the last end of a block was a grant that the person has not answered.
- A person who opens the microphone on a device whose report does not reach the server keeps the mark. The client keeps the rule of ADR 0785: an open by the person after the last state that showed the grant is not undone by a later state.
- Tests: `calls_grant` in the API tests, among them `a_switch_of_device_during_the_meeting_gives_a_seat_with_the_block_and_no_grant`.
