# 0739. The rules of a call in the desktop client

## Status

Accepted

## Context

The server has the rules of a call: meeting mode with raised hands (ADR 0505, ADR 0512), microphone and sound blocks and the removal of a person (ADR 0503), the lock, the user limit and the setting that hides who is inside (ADR 0504, ADR 0514), requests to enter and invitations (ADR 0514), and moves between voice channels (ADR 0511). The client holds the roles of a workspace and no override of a channel (ADR 0719), and the state of a server holds an invitation or a request only while it waits. The media server of this project has no move function: the device joins the room of the destination by itself (ADR 0511). The presenter and a moderator with "mute members" lower the hand of another person with the route of ADR 0516.

## Decision

Requests and refusals:

- While the media is connected but the server has not admitted the current place, the call shows "Entering call…". Invitations, answers to entry requests, recording controls, the hand and room settings are not offered. Their action handlers also require the current calls view to be connected and admitted, with the target and identity captured when the control was rendered, so an earlier view cannot send an inside-only request during entry or into a replacement call.
- Each control asks the server and changes nothing on screen by itself. The change arrives with the state of the server.
- A request that the server refuses is told by one notice in the notice area of the shown view, with the sentence for the code of the refusal, until the person closes it, the next request starts, or the call ends. A refusal that arrives after a later request has started, or after the call of its request has ended, is not told.
- A control whose request the roles of the person do not allow is drawn disabled, and its tooltip states the missing permission. The roles are a prediction: the server decides, because an override of the channel can differ.

Meeting mode:

- The header of the voice channel of the call has "Meeting mode" with "On" or "Off", and the Settings tab has the same switch. It turns meeting mode on for a person who shares, has "run meeting mode" and has no microphone block; in the other cases it is disabled and states why. While another person runs meeting mode it is disabled and names that person.
- While meeting mode is on, a bar in the reserved notice area at the end of the stage column states that it is on and who runs it.
- The presenter sees in the bar the first person with a raised hand and a closed microphone, the number of further ones, and "Allow". "Allow" removes the microphone block of that person. After that the bar names a person who was allowed and has not opened the microphone.
- The presenter also sees "Lower hand" for that person, before "Allow". It lowers the hand with the route of ADR 0516, and the block of the microphone stays. The next raised hand then takes its place in the bar. Both buttons are at the end of the bar, after the part that holds the name and the count, so that a name of another length moves neither of them (ADR 0730).
- While another person runs meeting mode, the end of the microphone block of a person does not open the microphone: the calls of the client set the person's own microphone setting to "closed" in the same step and lower the hand of the person. This holds for a block of any reason and whoever removed it, the presenter or a moderator: the presenter removes a block whatever its reason (ADR 0512), and the state of the server does not tell who removed a block. It holds for a block that ends with a new token after a lost connection too. The bar shows "You may speak" and "Unmute", and "Unmute" opens the microphone. While the window shows another channel, a notice with the same button tells it.
- Outside that case, when a microphone block of the person ends and the person's own setting is "open", the people in the call hear the person again without an action of the person. A notice tells the person so. The block ends on the device when the event of its end is read; when that event was not delivered, the calls set the person's own setting to "closed" as for a grant, and a notice tells the person that the app could not read whether the person was allowed to speak (ADR 0785). This holds for the end of meeting mode (ADR 0505) and for a block that a moderator removes without meeting mode. Nothing is told with push-to-talk, where the microphone sends only while the person holds the key, and nothing while the device cannot use a microphone.
- The strip of the call has the hand button. It raises and lowers the hand in every call of a voice channel. A direct-message call has no hand.

The room:

- The header has "Lock room" with "On" or "Off", and the count of the people inside with the user limit. The Settings tab has "Room · name" with meeting mode, the lock, the user limit and "Hide who is inside". A call keeps the hide setting of its start (ADR 0517): while the setting of the channel differs from the one of the call, the row states that the change holds from the next call.
- The user limit has a button that lowers it and one that raises it. A limit of 1 that is lowered is removed. A voice channel without a limit gets the number of people inside as its first limit.

A room that a person cannot enter without help:

- A voice channel is closed for a person outside when it is in use and locked or full, or when the server refused the last join of the person as locked or full. The second case tells it for a voice channel that hides who is inside.
- A voice channel in use whose call the server does not show is drawn as hidden, whatever the setting of the channel is now.
- The row of the person of this device shows "connecting" until the server lists the person in the call. The server lists a person when its admission check has seen the device in the room, and it takes an action of a person inside (the lock, an invitation, the answer to a request, meeting mode) only from then on (ADR 0517).
- Below the scrolling channel list, a bounded scrolling notice area shows the selected closed voice channel with its name, reason and "Ask to enter". It names another full voice channel on one line. Entry notices therefore consume space from the bottom of the list without moving its rows.
- The box shows the request of the person: on its way, waiting with "Withdraw request", denied, or without an answer, the last two with "Ask again". An acceptance enters the call at once.
- A person has one request at a time. A new request withdraws the earlier one, and so does the entry into any call: an acceptance does not replace a call that the person entered meanwhile. An answer that arrives before the server has told the identifier of the request is kept and used when the identifier is known. A request without an answer is shown as ended after the time that the server gave it and 5 seconds.
- A request of the account that another device made is shown at its voice channel as waiting, with "Withdraw", from the list of the state of the server (ADR 0719). A request of this device that waits or is on its way comes first; a request of this device that was denied or ended does not hide the waiting request of another device, and "Withdraw" then puts away that end and withdraws the other request. A request that the state of the server still lists after this device saw it end is not shown as waiting. Its answer ends it on this device without a notice: an acceptance lets in the device that asked, and this device does not enter. Reason: the server keeps one request per account in a call, so a device that did not show the request offered "Ask to enter", and the server refused it with `conflict` while the request waited. Test: `a_person_asks_to_enter_a_locked_room_and_sees_a_full_and_a_hidden_room` in the window live tier and `a_room_that_is_locked_or_full_tells_a_person_outside_and_shows_the_request_to_enter` in `crates/client`, which also failed while an ended request of this device hid the waiting one ("Ask again" was offered, and the server refused it with `conflict`).
- A full room has the request too, as the server has it.

Prompts:

- A person inside sees a request to enter as a notice in the notice area of the shown view with "Let in" and "Deny", whatever channel the window shows.
- An invitation for the person is a notice with "Join" and "Decline", whatever server and channel the window shows. "Join" accepts it and enters the call, in place of a call that the device is in.
- The application remembers each invitation that the person made on this device, from the answer of the server, for the time that the invitation can wait and 5 seconds, and tells the person when it was declined or got no answer. The invited person can answer before the answer of the server reaches this device. While an invitation of the person is on its way, the application keeps the end of every invitation that it does not know, and uses the one with the identifier of the answer when the answer arrives. It discards the kept ends when no invitation of the person is on its way.

Inviting:

- The Members tab of a call has "Invite a member" under its list. It opens a list of the members of the workspace who are not in the call, with a field that searches it. "Invite" invites; an invited member shows "Invited" and a button that withdraws the invitation.
- The list holds every member of the workspace outside the call. The client does not know who has access to the voice channel; the server refuses a member without it.

The menu of a person:

- A row of a person in a voice channel of the channel list opens a menu: with a right click, and from the keyboard, while the row holds the focus, with the Menu key, Shift and F10, Enter or Space. The row of the person of this device has no menu.
- The menu has "Invite to my room" while the device is in another call of the same workspace, "Move to…" with the other voice channels for a person with "move members", the volume of the person for this listener, and under "Moderation" the microphone block with "mute members", "Lower hand" with "mute members" or for the presenter of meeting mode inside that call, the sound block with "deafen members", and "Disconnect" with "disconnect members". A block that exists is offered for removal.
- "Lower hand" lowers the hand of the person and changes no microphone block (ADR 0516). While the hand of the person is not raised, the item is drawn disabled and states that no hand is raised: a hand that is raised or lowered while the menu is open moves no item under the pointer.
- The menu offers the sound block of ADR 0503.
- A moderator acts on a person in any voice channel that the moderator can see, also from outside it (ADR 0514).

A move of the person of this device:

- When the server asks the device to move, the calls of the client ask for the token of the move and connect to the room of the destination with it, beside the call of the source. That connection sends and plays nothing. It is given 15 seconds; a failure is reported to the server.
- When the server tells that the move is completed, the connection is closed and the device enters the call of the destination as it enters any call, which ends the call of the source on the device. A notice tells the person that a moderator moved them.
- The connection to the room of the destination is closed with a request to the media server, when the move has its outcome, when the call ends and when another move replaces it. A connection that is only dropped stays in the room.
- The outcome of a move can be among the changes that a snapshot replaced. After a snapshot, a move that the state no longer holds is decided from the state: completed when the account is in the call of the destination, without effect otherwise. A move without an outcome after 45 seconds is given up.

The notice area is the fixed scrolling area at the end of a call stage, the area above a message list and under its direct-call controls, or the part below the actions of a state panel (ADR 0730).

## Consequences

- A person who shares can press "Meeting mode" before the server has seen the share. The server refuses for about one second after the start of a share, and the refusal is told.
- A person whose microphone block ended while another person ran meeting mode has the own microphone setting "closed" afterwards, also after meeting mode has ended. A moderator who removes a block during a meeting gets the same result as the presenter.
- The request that lowers the hand of a person who was allowed to speak is sent once and is not tied to the hand of that moment: a person who raises the hand again within the time of that request has it lowered.
- A request to enter whose answer was lost while the event stream was interrupted is shown as "nobody answered", also when a person inside accepted it. The server has no route that reads the outcome later.
- The count that decides "full" on screen holds the people inside. The server also counts people on their way in by a move (ADR 0514): for that moment a join can be refused as full before the screen shows it, and the refusal then shows it.
- A moderator with "mute members" sees "Lower hand" in the menu of every person, also of a person whose hand is not raised.
- A person without "manage channel" in the roles who has it by an override of the channel sees the user limit and the hide setting disabled. This holds until the client reads the overrides of a channel.
- The list of the members who can be invited can hold a member whom the server refuses.
- A person outside a voice channel that hides who is inside sees the request to enter only after a join that the server refused.
- An invitation that the person made on another device ends without a statement on this device.
- During a move the device holds two media connections for a moment, and the people of the destination hear the person only after the second join.
- A direct-message call has no room: its page has no room controls, no meeting mode, no hand and no invitation of members (ADR 0501, ADR 0735).
