# 0517. Who is inside a call, and the setting that a call keeps from its start

## Status

Accepted

## Context

ADR 0510 states two things about a member of a call. "A person is in a call for others when the record allows the identity and the media server lists it", and a member that has asked for a place and has not connected is not shown. ADR 0514 defined "inside" for a voice channel that hides who is inside as "the record of the call has a member row for the account". The two together gave an account that asked for a place and never connected to the media server every read of a hidden call, while the people inside did not see the account and got no event: the participants, the lock, the meeting, the requests to enter, the recordings of the running call with their transcripts, and the private system lines. A repeated request for the same place moved the end of the place each time, so the state had no end. The answer of the join of a move returned the call of the destination without this rule.

The event of a call named its voice channel and not its call. The delivery asked whether the account was in the call that the channel had at that time, so an account in a later call received the events of an earlier call of the same channel from the kept log.

ADR 0604 named one more case: a person with "manage channel" outside a hidden voice channel turns the setting off during a call and reads who is inside.

## Decision

Seat and inside:

- A seat is a member row of a call: an account asked for a place and got one. A seat counts for the user limit (ADR 0514), for the rule of one call per account (ADR 0510), for the own call of the account (`GET /api/v1/call`, `own_call_changed`), for the join token and for the admission check (ADR 0509).
- An account is inside a call from the transaction in which the admission check records that the media server lists its seat, until its member row is removed. The same transaction records `call_participant_joined`, so the people inside learn of the account at the moment from which it reads.
- A member whom the media server no longer lists stays inside while the time of absence runs (ADR 0509): the other people still see that member as a participant.
- Only an account that is inside:
  - is shown as a participant;
  - receives the events for the people inside (`call_entry_requested`, `call_entry_resolved`) and gets the requests to enter from `GET /api/v1/call` and from the answer of a join;
  - reads a call that hides who is inside: from the route of the call, from the snapshot, from `GET /api/v1/call`, and from the events of the call;
  - reads the recordings of a running call that hides who is inside, with their segments, files, transcripts, hits of the search and audit entries, and downloads an export file that holds such a recording (ADR 0604, ADR 0411);
  - is a reader of the private system lines of the call (ADR 0408);
  - acts as a person inside: the lock, the answer to a request to enter, an invitation, meeting mode, a recording, and the moderation of a call that hides who is inside (ADR 0514).
- The person who runs meeting mode removes a microphone block and lowers a hand without "mute members" (ADR 0512, ADR 0516) only while inside the call. The meeting keeps its presenter for 60 seconds after the presenter left; in that time the presenter has no right from the meeting.
- One database function, `is_inside_call(call, account)`, states the rule. Every route, every statement and every trigger that asks whether an account is inside a call uses it. Two places read the member rows of one call themselves and state the same condition on the row: the list of the participants of a call, and the trigger for a member row that was removed, which the function can no longer read. A second function, `reads_call(call, account)`, states who may read what tells who is inside: the call does not hide who is inside, or the call has ended, or the account is inside.

What an account gets before it is inside:

- The answer of a join, of an accepted invitation and of the join of a move, and `GET /api/v1/call`, carry the call as the account may read it at that moment. For a call that hides who is inside and an account that is not inside, the call has no participant, no lock, no meeting and no recording. The field `hides_occupants` of the call states that the call hides them.
- A client reads `GET /api/v1/call` when `call_participant_joined` names its own account. That event is the first one of the call that the account receives.
- The join token of an account that is not inside a call that hides who is inside allows pictures only, whatever the state of the call. What an account may publish depends on meeting mode (ADR 0512), so the token of a seat would tell whether meeting mode is on. The admission check sets what the account may publish in the transaction and the requests in which it first sees the account in the room, as it sets it at every check (ADR 0509).
- The answer of a join, of an accepted invitation and of the join of a move states no microphone block and no sound block for an account that is not inside a call that hides who is inside (ADR 0510 states them in every other answer). A block names the account that set it, the block of meeting mode tells that meeting mode is on, and a moderator inside can set a block for a seat that never connected. The account reads its blocks with the call: from the event of its own entry and from `GET /api/v1/call`.
- A repeated request for the same seat by the same session returns a new token and does not move the end of a seat that has not connected. A request after that end is a new entry: the earlier seat ends with the reason `never_connected`, the lock and the user limit apply again, and the account gets a new identity.

The setting that a call keeps:

- A call stores the setting "hides who is inside" that its voice channel had when the record of the call was created. The database copies the value and keeps it: a statement that states another value for an existing call changes nothing.
- Every rule about a call reads the stored value: the reads above, the events, the speaking marks (ADR 0513), the private system lines, the recordings.
- A change of the setting of the channel during a call changes nothing of that call. It applies to the next call of the channel. The channel carries the new setting at once, and `channel_updated` and the audit entry are recorded as before.

Events of a call:

- The audience of an event about the inside of a call names the call.
- An event for the people inside is delivered to an account that is inside that call at the delivery. After the end of the call it is delivered to nobody.
- An event for the accounts that may see who is inside is delivered, for a call that hides who is inside, to an account that is inside that call at the delivery; for another call, to every account that may see the channel, also after the end of the call.
- The stored state of a call stays in `call_message_privacy` after the end of the call, so that the delivery decides an event of an ended call by the state that the call had. An event of a call without such a row is delivered to nobody.
- The migration gives each stored event of this kind the call of its payload.
- The migration also corrects the calls that run at the upgrade: an account with a seat that the check has not seen, and of whom no stored event states an entry into that call, is taken from the readers of the private lines of that call, in the lines and in the audiences of their stored events. The lines of calls that ended before the upgrade keep their readers, because no record states who of them was seen in the room.

## Consequences

- An account that asks for a place in a hidden voice channel and does not connect learns what a person outside learns: that the channel is in use, and whether it is locked or full.
- A person with "move members" who moves the own account towards a hidden voice channel reads nothing of it before the admission check has seen that account there, and then the people inside see the account.
- A device that enters a hidden voice channel shows the people inside one request later than before: it reads the call after the event of its own entry.
- A device that enters a hidden voice channel cannot publish its microphone before the admission check has seen it, which is at once after the notification of the media server and at the latest at the next full check (ADR 0509). A client publishes the microphone when the media server reports the changed permission, as it does when a microphone block ends.
- A person with a sound block who enters a call that hides who is inside again is told the block with the event of the own entry, not with the answer of the join. The client keeps the sound of such a call off until then (ADR 0734).
- A person inside a call receives the requests to enter only when the media server lists that person. A client that connects late answers no request before that.
- An account that enters a later call of a voice channel receives no event of an earlier call that hid who is inside, and nobody receives the requests to enter of an ended call. ADR 0011 states that an account reads the kept log for the objects that it may see now; a call that hid who is inside is not such an object for an account that was not inside it.
- A person outside cannot make a running call visible. A person inside who wants the call to be visible ends it and starts a new one after the change.
- A call that began before the setting was turned on stays visible until it ends. The people inside see the new setting on the channel and `hides_occupants: false` on the call.
- The delivery of a batch reads one more statement when the batch holds an event about the inside of a call: the state of the calls that the batch names, through the primary key.
- A device that was accepted or invited and waits longer than the 120 seconds in which a seat is usable gets a new seat with its next request; when the channel was locked in between, the request is refused as for a person outside.
