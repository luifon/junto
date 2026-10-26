# 0510. Call state: the record, who is in a call, one call per account, blocks and events

## Status

Accepted

## Context

ADR 0501 to ADR 0507 decide what a call is and what its rules are. They leave open how the server stores a call, how "who is inside" is built from the media server and from the server's own record, how the rule "one call per account" (ADR 0107) is kept when a second device joins, how blocks survive a reconnect, and which events a call records for whom. Access to a voice channel has no permission of its own in ADR 0301.

## Decision

Access:

- An account may enter a voice channel when the permission function allows it `view_channel` there. There is no separate permission to connect. ADR 0301 has none, a voice channel that a person sees and may never enter has no use in the product, and a place where some people may not enter is expressed with a private channel, a lock or a user limit (ADR 0514). One permission fewer is one rule fewer that the check, the events and the snapshot must agree on.
- An account may call another account in a direct message while the two may exchange direct messages: both are active, and they have a common workspace or one of them administers the server (ADR 0307). Seeing the profile of an account (ADR 0119) does not allow a call.

The record:

- A call exists from the first request to enter until it has no member. A voice channel has at most one call, and a pair of accounts has at most one.
- A member row states the account, the device and the session that asked, the identity that is allowed in the room, and the person's own state.
- An account has at most one member row on the whole server. The database enforces this, the identity of a row names its account, and a member of a direct-message call is one of its two accounts.
- A seat belongs to one session. A request to enter by another session gets a new identity, also when it comes from the same device. The end of a session removes its member row in the same transaction (ADR 0509).
- A request that reads or changes the caller's own call needs the caller's current access to the place of the call, decided in the transaction of the request. An account that has lost the access gets no call from `GET /api/v1/call` and `not_in_call` from the routes that change its state, also before the admission check has removed its row.
- A request that changes the caller's state in a call, reports who speaks, or asks for the token of a move comes from the session that holds the seat. Another session gets `not_in_call`.
- A request that changes a call locks the row of the call and raises its revision. Two requests that change the same call never both commit from the same state: one runs again (ADR 0009).

Who is in a call:

- A person is in a call for others when two things hold: the record allows the identity, and the media server lists it. The admission check records the first sighting (ADR 0509).
- A member who has asked for a place and has not connected is not shown. It takes a place for the user limit. It is not inside the call: ADR 0517 states what a seat alone gives and what only a person inside reads and does.
- A person leaves the call for others when the member row is removed.
- A voice channel is "in use" from the first sighting of a participant until the call ends.

One call per account, and the transfer rule:

- A request to enter by the device that holds the place returns a new token for the same identity. It does not move the end of a seat that has not connected; a request after that end is a new entry (ADR 0517).
- A request that names the seat that it renews (`renews`) gets a token for that seat or `seat_ended`, and makes, ends and takes no seat (ADR 0520).
- A request to enter the same call by another device of the account moves the call to that device: the earlier identity is no longer allowed, the account keeps its place, and the lock and the user limit do not apply. Others see the person leave and join.
- A request to enter another call ends the membership in the first call. The lock and the user limit of the new place apply.
- The earlier identity is removed from its room by the admission check.
- The request that leaves a call can name the identity of the place that the device leaves. The server then ends the place only when the account holds it with that identity; with another identity, or without a place, nothing changes and the answer is the same. A device that leaves late does not end the place that another device of the account took. The identity of the seat that a completed move replaced names the seat that the move gave (ADR 0511). Without an identity the request ends the call of the account from whichever device.

Own state and blocks:

- The device in the call reports the person's own microphone and sound settings. They are shown as `muted` and `deafened`.
- A block of a moderator is shown as `microphone_block` or `sound_block`, each with the account that set it. The four fields are independent: removing a block leaves the person's own setting as it is.
- A block belongs to the call and to the account, not to the stay of the account. A person who leaves and returns, or moves the call to another device, keeps the block.
- A microphone block closes every sound source: the microphone and the sound of a screen share. The media server forwards the sound of a share as an audio track that the sharing device fills, and the server cannot verify that it carries shared sound and not a voice. A block that left this source open could be passed by a modified client. The camera and the screen stay open.
- The block is in the join token of a blocked person and is set again at every admission check.
- The answer to a request for a place states the microphone block and the sound block of the caller. A call lists a person only after the device has connected, so a device that enters again would otherwise learn its blocks after its connection, and a sound block, which the client applies, would not hold in between. For a person on the way in by a move, the answer states the block of meeting mode that the person gets at the completion. For an account that is not inside a call that hides who is inside, the answer states no block (ADR 0517): a block names the person who set it, and the block of meeting mode tells that meeting mode is on. That account reads its blocks with the call when the admission check has seen it, and its client plays no sound of the call before that (ADR 0734).
- A participant with a microphone block whom the media server lists with an audio track, of whatever source, is removed from the room.
- A sound block is a flag. The client applies it (ADR 0503).
- A block ends with the call.

Screen share:

- A person runs up to three shares (`MAX_SHARES`, ADR 0502). Each share has a slot from 0 to 2 that the sharing device chooses. The table `call_shares` holds one row per share: the call, the account, the slot, what it shows (`screen` or `window`, absent from a device that does not state it), its sound, its start, when the admission check first saw its picture track, and since when it misses. A row ends with its member.
- The device reports a share with `PUT /api/v1/call/screen-share`: its slot, what it shows, and which sound it sends: `application`, `system` or `none`. Another sound, and a slot of 3 or more, are refused. A report without a slot is slot 0. A report of a slot that the server holds with the same sound and kind changes nothing; another sound or kind changes the row and keeps its start.
- `DELETE /api/v1/call/screen-share?slot=N` ends one share; without a slot every share of the person ends.
- The share is shown from the report. The picture track of a share names its slot (ADR 0737): `screen/30` or `window/15` is slot 0, `screen/30/1` and `window/30/2` are slots 1 and 2, and every other name counts as slot 0. The admission check ends a share when the media server has listed no picture track with the slot of that share for 10 seconds; the other shares of the person are not touched.
- The server cannot verify the sound choice. It verifies that the picture track of each share exists.
- A participant carries the list `screen_shares` in the order of the slots, and `screen_share`, the share of the lowest slot. A client of v0.1.0-test.1 or test.2 reads only `screen_share`, reports without a slot, ends without a slot and names its track `screen/30`: it keeps working with one share. A client of several shares that talks to a server from before reads `screen_share` alone when `screen_shares` is absent.
- Each report or end of a share that changes the record, and each end by the admission check, publishes `call_participant_updated` with the whole participant, so the event of a change of one share carries every share of the person. Each share that starts in a voice channel writes the line `screen_share_started`; two shares of one person write two lines.

Events:

| Event | Recipients |
|---|---|
| `call_started`, `call_ended` | Accounts that may see the voice channel; the two accounts of a direct message |
| `call_participant_joined`, `call_participant_updated`, `call_participant_left`, `call_lock_changed`, `call_meeting_started`, `call_meeting_ended`, `call_speaking` | Accounts that may see who is inside (ADR 0514); the two accounts of a direct message |
| `call_entry_requested`, `call_entry_resolved` | The accounts inside the call, while the call exists (ADR 0517) |
| `call_entry_requested` | Also the account that asked, while it may be at the place of the call, so that its other devices learn of the request that the snapshot carries |
| `call_entry_answered` | The account that asked, while it may be at the place of the call |
| `call_invited`, `call_invitation_resolved` | The invited account and the inviting account, each while it may be at the place of the call |
| `call_move_requested`, `call_move_resolved` | The moved account and the account that moved, each while it may see both voice channels |
| `own_call_changed` with a call | The account itself, while it may be at the place of the call |
| `own_call_changed` without a call | The account itself |
| `call_access_ended` | The account itself |
| `direct_mute_set`, `direct_mute_removed` | The account itself (ADR 0515) |

- Each event carries the whole changed object, as the events of the server base do (ADR 0208).
- "May be at the place of a call" means: for a voice channel, the account may see the channel; for a direct message, the account may exchange direct messages with the other account (ADR 0307).
- An event that is addressed to one account and is about a call is delivered only while that account may be at every place that the event is about. The decision is made at delivery and at replay (ADR 0011), so an invitation that was recorded before the account lost the channel does not arrive after it.
- The events of a direct-message call reach its two accounts only while the two may exchange direct messages, by the same decision at delivery.
- The outcomes `call_entry_answered`, `call_invitation_resolved` and `call_move_resolved` name the request, the invitation or the move, and no place. They are still delivered only while the account may be at the place: an outcome with a known identifier tells an account that somebody entered, declined or left, also without a name.

When an account loses a place:

- The admission check (ADR 0509) ends, in its transaction for the call, everything that waits there for an account that may no longer be at the place of the call: the invitations that the account got or made, its request to enter, and a move of it or by it. A direct-message call whose two accounts may no longer exchange direct messages ends in the same way.
- The account gets one event `call_access_ended`. It has no field. The client discards what it holds for the places that it can no longer see: invitations, requests, moves and direct-message calls.
- No outcome of those things is sent to that account, then or later. The other side of an invitation gets `call_invitation_resolved` with `cancelled`, and the people inside get `call_entry_resolved` with `cancelled`.
- A member of the call who loses the place also gets `own_call_changed` without a call, as for every end of a place.
- `call_invited` and `call_invitation_resolved` have the class of call invites and reach every connection level (ADR 0107). The events of moves, `call_entry_answered`, `own_call_changed`, `call_access_ended` and the events of mutes are control events. The others are general.
- A connection whose account has a seat in a call receives every event of that call at any level. A background connection reads the seat of its account in the transaction of each batch, so the classes that it reads are those of the record at that moment. The server also keeps the accounts in calls in memory (ADR 0116); every admission check replaces the list with the content of the record. That list decides only which new events wake a background connection, and a replacement wakes the connections of the accounts that it adds. It never decides an access, and it no longer decides which classes a batch reads: a list that was behind the record let a background connection pass events of its own call for good.
- The snapshot (ADR 0207) carries the call of the account, the calls of the voice channels that the account may see, the direct-message calls of the account that are in progress, the invitations that wait for the account, the invitations that the account made and that wait, the moves that wait and that name the account as the moved person or as the person who moved, and the requests of the account to enter that wait. Each of the last four is carried only while the account may be at every place that it names. A device that had to load a snapshot learned of none of the last three before.

Kept for the text chat:

- The server keeps a row for each of these in the call of a voice channel: the start and the end of the call, the start of each screen share, a request to enter, a person who entered by invitation, and the start and the end of meeting mode. The text unit shows them as system lines (ADR 0501).

Limits of requests:

- One session sends at most 120 requests per minute that change a call, and at most 30 reports of its own state per 10 seconds. A further one gets `rate_limited`.
- The limit of 120 covers every route that can change a call: enter, leave, lock, meeting mode, hand, requests to enter and their answers, invitations and their answers, moves and their answers, blocks, and removals. A request counts whether or not it changes something.
- A request that changes nothing starts no admission check (ADR 0509).

## Consequences

- The view of a call follows the media server with a delay of at most one admission check.
- The person's own mute state is a report. A modified client can report "muted" and send sound. A microphone block does not depend on a report.
- A blocked person cannot share the sound of a screen.
- A person who is blocked can end the block only by the end of the call, by a moderator, or by the end of meeting mode (ADR 0512).
- A device that is in a call and whose account joins from another device learns it from `own_call_changed` and is disconnected by the media server.
- A device that logs in again while it is in a call has a new session. It enters again and gets a new identity; others see the person leave and join.
- An account that loses a voice channel while it is in the call learns that its call ended from `own_call_changed` without a call. It does not learn who stayed.
- Between the loss of a place and the next check of the room, the account receives nothing about the place: the delivery decides each event. The event `call_access_ended` follows at that check, at a time that depends on the check and not on what other people do.
- A client that receives `call_access_ended` cannot tell from it which place ended. It compares what it holds with the channels and accounts that it can still see.
- An account without a common workspace with another account cannot call it, unless one of the two administers the server (ADR 0307).
- The audit log of a workspace shows the moderation in a hidden voice channel to its readers, with the accounts concerned.
- The list of accounts in calls is per process (ADR 0011).
