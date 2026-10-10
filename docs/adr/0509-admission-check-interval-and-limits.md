# 0509. Admission check: interval, triggers and known limits

## Status

Accepted

## Context

ADR 0506 decides that the server compares the participants of every room with the people allowed, at short intervals and whenever its connection to the media server returns, and leaves the interval open. Step 0 measured why one check at the join is not enough (`docs/step-0/README.md`, test 5): a join token of 60 seconds was accepted 54 seconds after its end, the media server gives each connected client a token of 600 seconds, and a removed participant joined again with both. The notifications of the media server can be lost, and a stop of Redis makes the API of the media server fail (`spikes/media-host/RESULT.md`, M6).

## Decision

The record:

- The server keeps its own record of who is allowed in each room: one row per call, and one row per member with the one identity that is allowed for it (ADR 0510).
- Every rule about who may be in a call changes this record in the transaction of its request, and nothing else: a join, a leave, a removal by a moderator, a lock, a user limit, a move.
- The end of a session removes the place of that session in a call, and a move of it that waits, in the transaction that ends the session: a logout, a removed device, a ban, a removal of the account, a recovery, and the expiry of the session. The place does not wait for a check. Without this rule, a new session of the same device could take over the place before a check saw the end, and a holder of the old token would stay allowed.
- The admission check makes the rooms agree with the record. No request removes a participant itself.

The check of one room:

- The check asks the media server for the participants of the room.
- For the call of the room, in one transaction (ADR 0009), the check:
  - decides for every member with the permission function (ADR 0301) whether its account may still be at the place of the call, and whether the session that joined still counts; a member that fails loses its row;
  - ends the invitations, the requests to enter and the moves that wait for an account that may no longer be at the place of the call, and records `call_access_ended` for that account (ADR 0510);
  - records a member that the media server lists for the first time as in the call (ADR 0510);
  - removes the row of a member that was in the call and that the check has seen absent for 20 seconds, and of a member that never connected within the 120 seconds in which its join token is usable;
  - completes or ends the moves into the call (ADR 0511), ends requests and invitations that waited too long (ADR 0514), and applies the timings of meeting mode (ADR 0512);
  - ends the call when it has no member.
- After the transaction the check removes from the room every participant whose identity the record does not allow, and replaces what a participant may publish when it differs from the record.
- The removals are sent one after the other, and the changes of what a participant may publish are sent one after the other. The two sequences run at the same time. A removal that is slow or fails therefore takes no time from the changes, and a change that is slow or fails takes no time from the removals. A microphone block reaches the media server also while a removal in the same room does not complete.
- The check skips the participants of the recorder (ADR 0506).
- The check removes every participant of a room that has the namespace of this server and belongs to no call (ADR 0508).
- A participant with a microphone block that publishes an audio track is removed from the room (ADR 0510).

The full check, in parts:

- The full check takes the rooms in turn. One part of it runs at the start of the server and every 2 seconds.
- A part asks the media server for its rooms. Then it checks the next 50 rooms with a call and the next 20 rooms of this server without a call, in the order of the room names. The part after it continues behind the last room that this part took, and after the last name it continues at the first.
- The server keeps the two positions in memory. After a restart the turn starts at the first name.
- A part checks at most 10 rooms at one time.
- The check of one room has a time limit of 5 seconds, for its request, its transaction and its actions together. Each of the two sequences of actions runs until the end of these 5 seconds. A room that passes the limit has failed for this check. The actions that were not sent are computed again at its next check.
- A verified notification of the media server that names no room makes the next part due at once (ADR 0508).

The targeted check:

- A request marks the room of each call whose record it changed. The marks are applied when the request has ended, which is after its commit, so a check never looks at a room for a change that the database does not have yet.
- The targeted check runs at once after such a request: the call routes, a ban, a removal, a logout, a device removal, a recovery, a deleted channel and a changed channel override. It runs at once after a verified notification that names a room, for that room.
- A request that changes permissions marks the rooms that the change can concern: the calls of the workspace for a change of its roles or of the roles of a member, and every call for a change of server roles or of the owner. A person who loses a place by such a change is taken out at once, and what waited for that person ends at once (ADR 0510).
- One run checks at most 50 marked rooms, 10 at one time, each within the 5 seconds of one room. Further marked rooms are checked by the next run, which follows at once, or after one part of the full check when that part is due.
- A request that changes nothing marks no room and starts no check. Without a marked room the targeted check sends no request to the media server. No request of a client starts a part of the full check.
- The parts of the full check and the runs of the targeted check run one after the other in one task. When a part is due and a run is asked for at the same time, the kind that did not run last goes first. A run of the targeted check therefore never waits for more than one part, also while every part takes longer than the interval, and a part never waits for more than one run.
- The server remembers at most 4096 marked rooms. With more, it drops the marks: the full check reaches every room in its turn.

How long a room waits:

- R is the number of rooms with a call, B = 50 the rooms with a call of one part, I = 2 seconds the interval, and T the time that one part takes.
- A room with a call is checked at least once in every `ceil(R / B)` parts that follow each other. It waits at most `ceil(R / B) × max(I, T)` for the full check, plus the time of the targeted runs that delay those parts, which is at most one run for each part.
- With a media server that answers, T is shorter than I: a room waits at most 2 seconds with up to 50 calls, 4 seconds with 100 calls and 40 seconds with 1000 calls.
- T is bounded: at most 2 seconds for the request for the rooms, and `ceil(70 / 10) × 5` seconds for 70 rooms that all pass their time limit, 37 seconds together. A run of the targeted check is bounded in the same way by `ceil(50 / 10) × 5` = 25 seconds.
- A room without a call is checked at least once in every `ceil(E / 20)` parts, with E such rooms.
- A room that a request changed does not wait for its turn: the targeted check takes it.

Failures:

- A failure is kept to its room and to its action. A room whose participants cannot be listed, or whose transaction fails, is left as it is, and the check goes on with the other rooms.
- Every removal and every change of a permission is sent by itself. One that fails does not stop the others, and the removals and the changes of permissions do not wait for each other.
- No action is stored. The next check of the room computes all actions again from the record and from the room.
- A room with a failed check lets nobody in: a join, an accepted invitation and a move into it get `media_unavailable` until a check of the room has succeeded with all its actions.
- The first failure of a room is logged as a warning. After 3 failed checks in a row the room is logged as an error and counted in `failing_call_rooms` of the health route (ADR 0012), until a check of it succeeds.
- A room that keeps failing keeps its place in the turn and takes no place of another room.

Time of absence:

- Only time in which the server observed the room counts for the 20 seconds of a member, the 10 seconds of a screen share (ADR 0510) and the 60 seconds of a presenter (ADR 0512).
- A room is observed when the media server has listed its participants, or has listed its rooms without this room, and the transaction that compares them with the record has committed.
- The server keeps for each room whether it was observed since the media server last became reachable and since the start of the server. A successful request for the rooms observes no room.
- The times of a call start again at an observation of its room when the room was not observed before in that time, or when an observation of it failed since: its participants could not be listed, its transaction failed, or its time limit passed before the transaction. A member who was absent before has 20 seconds again from then.
- A removal or a change of a permission that fails does not start the times again. The room was observed, and its members who are absent leave after their 20 seconds.
- The time between two observations of a room counts in full, also when several parts lie between them.

While the media server is unreachable:

- A part of the full check runs at each interval and changes nothing. The targeted check does not run, and the marked rooms stay marked. After the return, the marked rooms are checked at once and every other room in its turn.
- A request that would let a person into a room gets `media_unavailable` (503): a join, an accepted invitation, a move.
- Requests that take a person out are accepted. They change the record and take effect at the first check of the room after the return.
- The health route reports `"media_server": false` (ADR 0012). Its status does not change.

## Consequences

- Known limit: a removed person who holds a usable token can be back in the room after each removal. That person is in the room until the next check of the room: at most the wait that is stated above, and less when a notification arrives. During that time the person hears the call, and can speak unless a microphone block was in the token. The server cannot shorten this further, because a removal ends no token.
- Known limit: a person with a microphone block who reconnects with a join token from before the block can publish sound until the next check of the room, which sets the permission again.
- The limit ends with the call: the next call of the channel has another room (ADR 0508).
- Known limit: an outage is noticed at the next part. A join in the 2 seconds before that gets a token that the media server does not serve.
- Known limit: people who are in a room when the media server becomes unreachable for the server, and reachable for themselves, stay there, a removed person included, until the first check of the room after the return. This is the case of ADR 0506 for the two-host form.
- Known limit: a person whose removal from one room keeps failing stays in that room. The other rooms are not affected, and the health route and the log name the condition.
- Known limit: a change that no request of this server makes, such as a participant who connects again with an old token, is found by a notification or at the turn of the room. With more than 50 calls the turn comes less often than every 2 seconds.
- A lost notification costs at most the wait of one turn.
- Cost of one part: one request for the rooms and one statement for the names of the recorded rooms; then for each of at most 70 rooms one transaction, one request for its participants when the media server lists it, and one request for each participant to remove or to change. At most two requests about the participants of one room are in progress at one time: one removal and one change of a permission. The transaction evaluates the permission function once per member and once per account with an invitation, a request to enter or a move that waits, with about six statements each.
- A member who is absent while the media server is unreachable, or while the room cannot be observed, keeps the place for up to 20 seconds after the first observation. No person is removed for an absence that the server did not observe.
- A person who returned and left again between two observations of a room counts as absent for the whole time between them.
- The interval, the sizes of a part, the 10 rooms at one time, the 5 seconds, the 20 seconds, the 3 failures and the time limits are values of the server configuration in the code.
- The check runs in the one server process (ADR 0008). A second process would run a second check with the same result; its positions and its observations are its own.
- "Removed" is observed against LiveKit 1.13.9: a participant that returned with its token twice was disconnected by the check twice.
