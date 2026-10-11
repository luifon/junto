# 0719. Server state in the client: the model, the parts read on demand, and the interface for readers

## Status

Accepted

## Context

The interface shows, for each server, its workspaces, channels, the people in voice channels, the members, and what waits for the user: invitations, requests to enter and moves (ADR 0703, ADR 0501 to ADR 0515). The interface is built with gpui, whose thread must never wait for the network or for a lock (ADR 0701, ADR 0715). ADR 0207 defines the snapshot, the events that continue it, and the collections that the snapshot does not carry. Text messages are added in a later step (ADR 0903), and their state must fit without a change of what exists.

## Decision

The state:

- The client holds one state per server: the cursor, the account, the workspaces that the account can see, the call of the account, its direct-message calls, the invitations, requests to enter and moves that wait, and its muted conversations.
- A workspace holds its roles, the roles of the account, its categories, the channels that the account can see, and its members once a reader asked for them. A voice channel holds whether it is in use and what the account knows of its call.
- A state is built from a snapshot. One function applies one event to it. That function reads and writes nothing else, and returns what must follow: nothing, a reload of the state, or a read of one part.
- An event carries the whole changed object (ADR 0208), so the function replaces the object with the same identifier, and applying an event twice changes nothing. Lists keep the order of the server: channels and categories by position, the people of a call by the time of their join and then by account.
- An event about an object whose parent the state does not hold is not applied. The parent became visible by a permission change, and the reload after that change brings both.
- The state holds no devices, accounts, server roles, invites, join requests and channel overrides. Their events change nothing and are told to readers as notices.
- The fields of the account change by the event of its profile and by a reload.
- The state gives the permissions as far as the snapshot does: the server permissions of the account, and per workspace the permissions of the "everyone" role and of the roles of the account. Channel overrides are not in the snapshot, so the client has no answer for one channel. The server decides every request.

Calls in the state:

- The details of a call (people, lock, meeting, speaking marks) are created by the first event that names the call. `call_started` marks the voice channel as in use, and `call_ended` removes the call.
- A voice channel that hides who is inside (ADR 0514) has no details while the account is outside. When the setting is turned on, the details are dropped. When it is turned off while the channel is in use, the client reads `GET /channels/{id}/call`, because no event tells who is inside. The answer, and the events that were kept during the read, are used by the setting that the state holds when the answer arrives: for a channel that hides again, only whether it is in use.
- `call_meeting_started` carries no participant. The client applies the rule of the event: every participant except the presenter who has no microphone block gets one with the reason `meeting`. `call_meeting_ended` removes every microphone block (ADR 0512).
- `own_call_changed` with a call sets the call of the account, and the client reads `GET /call` for the people inside and the requests to enter that wait. Without a call, the requests to enter are dropped, and so are the details of a call that is not in use or whose channel hides who is inside.
- An invitation, a request to enter and a move stay in the state until their outcome event, the end of their call, the deletion of their channel, or a reload. The state keeps no list of outcomes: an outcome is a notice.
- A snapshot carries the invitations that the account got, and no move and no invitation that the account made (ADR 0207). A reload therefore removes those from the state while they still wait on the server. Their outcome is told as a notice when the server delivers it (ADR 0510). The requests to enter are read again with the call of the account.
- The request of the account itself to enter is in no event and in no snapshot. The caller has it from the answer to its request and waits for the notice `call_entry_answered`.
- `call_access_ended` names nothing, and the client cannot decide the contact rule of ADR 0307 by itself. The state is reloaded (ADR 0718).
- A snapshot carries no speaking marks. They are empty until the next `call_speaking`.
- The details of a call hold its recording while the recording is `starting` or `recording` (ADR 0601, ADR 0604). The snapshot and the two views of a call carry it. `recording_started` and `recording_updated` change it only for a call that the state holds with the same identifier: the people in a call start a recording, so the events of the call came first, and a replay can deliver the recording of an earlier call of the channel. A recording in another state leaves the details; its later changes are notices. `recording_removed` removes the recording from the call that shows it. The events of transcripts and of the glossary are notices: the state holds no transcript and no glossary.

Parts read on demand (ADR 0207):

- The members of a workspace are read when a reader asks for them. The part is `not loaded`, `loading` or `loaded`. The reader is answered after the state with the members was given to the readers, so that it finds them there.
- The call of the account and the call of a channel that stopped hiding are read by the client itself, as stated above.
- One rule for all of them: the events that arrive after the request was sent are kept, and after the answer they are applied to that part in their order. A read during which more than 4096 events or 4 MiB of events arrive starts again.
- A reload discards every part, because the client no longer knows which changes it missed. The members that a reader asked for are read again without a new request, until the reader releases them. The call of the account is read again.
- A read that fails gives its reader the error and leaves the part `not loaded`.

Copies for readers:

- The stream task (ADR 0718) is the only writer. After it has applied the frames that wait, at most 256 at a time, it gives the readers a new copy of the state that cannot be changed.
- Each workspace is shared between a copy and the next one as long as no event changed it. A reader that keeps an old copy costs the memory of the changed workspaces.
- `view` returns the status of the stream, the level and the newest copy at once.
- `watch` wakes a reader after a change and gives the newest view. Several changes can arrive as one.
- `notices` gives every applied event with its position and time, each reload, and an ended session, in their order. A notice is given after the copy that holds its change, so a reader that gets a notice finds the change in the state. The queue holds 1024 notices, which is more than one replay (ADR 0207). A reader that falls behind is told how many it missed and reads the state.
- The three work on any executor and take no lock that a request holds, so the thread of an interface does not wait (ADR 0715).
- The status is `off`, `connecting`, `live`, or `interrupted` with the cause. During `interrupted` the copy is the last known state.
- Every request of the account is a future of the API object of the server (ADR 0715). Its result is the value, a refusal with the error code of the server (ADR 0205, ADR 0209), or the statement that no answer arrived (ADR 0714).

Later parts:

- The messages are in the state as ADR 0721 states: the unread summary of each place, the timelines of the places that a reader has open, and the messages that wait to be sent, with new cases of the function that applies events and new parts. A timeline belongs to the state of the server and not to its channel, because a direct conversation has no channel. No field or case of before changed.

## Consequences

- An interface draws a copy and offers actions. It holds no rule about events.
- A reader that only draws the state needs `watch` alone. A reader that shows single changes, such as a ringing call or the outcome of a request, reads the notices and treats a missed notice by reading the state.
- At the background level the workspaces and channels are as old as the last snapshot, because the server delivers no change of the structure at that level (ADR 0107). The view states the level.
- After a reload the members are `loading` for the time of one request, and the speaking marks are empty until they change.
- A reader shows the recording indicator of a voice channel from the details of its call. An account outside a channel that hides who is inside holds no details and shows no indicator, as the server sends it none.
- A lock that was set before the first person of a call was seen is in the state before the channel is in use. A reader shows a lock only for a channel in use.
- The client reads one more route when the account enters a call, and one when a channel in use stops hiding who is inside.
- The function that applies events is tested without a server, for every event that it applies; the stream client and the real server are tested with it (ADR 0718).
