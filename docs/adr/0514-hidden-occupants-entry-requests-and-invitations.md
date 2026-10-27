# 0514. Hidden occupants, the lock, requests to enter and invitations

## Status

Accepted

## Context

ADR 0504 decides the privacy of a voice channel: a user limit, a lock, a setting that hides who is inside, requests to enter, and invitations. It leaves open what exactly a person outside a hidden voice channel can learn, what counts for the user limit, how long an accepted person can enter, and how long an invitation waits.

## Decision

User limit and lock:

- The user limit of a voice channel is from 1 to 99, or absent. It counts the members of the call and the people on their way in by a move (ADR 0511).
- A person inside a voice channel with "lock room" locks and unlocks it. The lock belongs to the call and ends with it, so an empty voice channel is never locked.
- The hide setting is changed with "manage channel". The user limit is changed with "manage channel" or with "set call user limit": a request that changes the user limit alone takes either, and a request that changes the hide setting takes "manage channel". The change is in the audit log as `channel_call_settings_updated`.
- A change of the hide setting during a call applies to the next call of the channel. The running call keeps the setting that it started with (ADR 0517).
- A person who is in the call keeps the place when the channel is locked or the limit is lowered.

Hidden occupants:

- A voice channel that hides who is inside shows the people outside one thing: whether it is in use.
- The people outside get `call_started` and `call_ended` and no other event of the call. The route of the call and the snapshot give them no participant, no lock and no meeting.
- A person is inside from the moment at which the admission check has recorded that the media server lists the seat of the account, until the member row is removed (ADR 0517). An account that asked for a place and has not connected is outside. A member row alone was the rule of the first version of this record; it let an account that never connected read the call while nobody inside saw it.
- The rule holds for every account outside, the server owner and administrators included. The setting is a promise to the people inside about who knows that they are there. A role that reads the occupants from outside would make the promise depend on who holds roles, which the people inside cannot see. An administrator who needs to know enters the voice channel, and is then visible to the people inside.
- A moderator who is outside gets `not_in_call` for every block, removal and move in that voice channel, whoever the request names. The answer does not depend on who is inside.
- The decision is made when an event is delivered (ADR 0011), with the setting that the call of the event started with and the people inside that call at that moment (ADR 0517). An event of an earlier call of the channel does not reach the people of a later call.

Requests to enter:

- A person with access to a locked or full voice channel asks to enter. One request per person waits, and at most 50 per call.
- The people inside get the request, while its call exists. Anyone inside accepts or denies. The first answer applies; a second one gets `conflict`.
- A request to enter a call that is full and not locked goes to the people inside who hold "admit to full call" in the channel (ADR 0301), administrators included, and only they answer it. When none of them is inside at the time of the request, everyone inside gets it and answers it, as for a locked call. The server records the accounts that answer with the request (`answerers`, null for everyone inside), delivers the request to each of them, lists it in the own call of each of them only, and answers `not_found` to an answer by another account. The set is decided at the request and is not changed while the request waits (60 seconds); a holder who leaves cannot answer, because an answer needs a place inside. A locked call keeps the rule for everyone inside, because the lock is a decision of the people inside. Reason: the owner of a voice channel asked that a person who knocks on a full channel is let in by the people who are responsible for it, and by everyone inside only when nobody responsible is there. Tests: `calls_rooms::a_request_to_enter_a_full_call_goes_to_the_holders_of_admit_to_full_call`, and in `junto-authz` `admit_to_full_call_is_held_by_a_role_an_override_or_an_administrator`.
- A designated answerer receives a request only while still inside its specific call at delivery time, including replay. The stored call audience restricts the account, indexes it in `events.audience_account_id` (migration `0034_account_call_audience`), and keeps the call key, so the request reaches a background connection while the account is in that call (ADR 0107). The requester receives a separate account notice for its other devices, subject to channel visibility; it does not require membership inside the call.
- A request without an answer ends after 60 seconds.
- An accepted person can enter once within 60 seconds of the acceptance. The entry passes the lock and the user limit.
- The person who asked gets the outcome in `call_entry_answered`, with the identifier of the request and without the account that answered, the channel or a person inside (ADR 0510).

Invitations:

- A person inside a voice channel invites an account that has access to the channel and that the person may see.
- An invitation waits 60 seconds. At most 50 invitations per call wait.
- One invitation per invited account waits in a call. A new invitation of the same account replaces the earlier one: the earlier one ends with `call_invitation_resolved` and `cancelled` for the invited account and for the account that made it.
- The invited account and the inviting account both get `call_invited`.
- The invited account accepts or declines; the inviting account can withdraw. Each of the three needs that the account may be at the voice channel at that time. An account that may not gets `not_found` and the invitation stays as it is, so the answer does not tell whether the invitation still waits.
- The acceptance checks the access of the invited account again and then enters the call. It passes the lock and the user limit, and no permission (ADR 0301).
- A direct-message call invites the other account in the same way when the first account enters (ADR 0501). That invitation has the class of call invites and reaches a background connection (ADR 0107). ADR 0515 limits these rings.
- An invitation or a request to enter of an account that loses the voice channel ends at the next admission check, without an outcome for that account (ADR 0510).

## Consequences

- A person outside a hidden voice channel learns that it is in use, and learns from the answer to a join whether it is locked or full.
- An invitation shows the invited account one person inside: the one who invited.
- A moderator must enter a hidden voice channel to moderate it.
- A person with "manage channel" who is outside a hidden voice channel cannot make its running call visible. The first version of this record applied a change of the setting at once, which let that person read who is inside.
- A person who was accepted and waits longer than 60 seconds asks again.
- An invitation into a voice channel that the invited account can no longer see is not delivered and cannot be accepted. The account gets `call_access_ended` and no outcome of the invitation.
- A voice channel can hold more people than its limit after an invitation, an accepted request or a move.
