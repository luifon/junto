# 0741. The thread panel, pins, permanent messages and direct conversations in the window

## Status

Accepted

## Context

The approved design draws a thread in a side panel beside the list of its channel, with the controls of a fork (view A), a channel whose messages are permanent (view J), and "Pins" in the header of a channel. It draws no direct conversation. ADR 0722 gives the window a thread as a place of messages, the unread threads, the pin list of a channel and the direct conversations of the state. ADR 0740 built one message list and one composer for the selected channel. ADR 0404 and ADR 0409 make the unread sign of a place depend on the notification settings of the person. The server refuses "also send to the channel" (ADR 0403), tells the count of the replies and the last reply of a thread and no other reply, and does not tell who pinned a message.

## Decision

Panes:

- The window has two panes of messages: the main area, and the thread of the side panel. Each pane has its own list, its own composer with its text, reply, edit and files, and its own read position. Every function of the list and of the composer names its pane. The menu of a message and the emoji picker of a composer are layers of the window and name their pane; a reaction and the question before a deletion name their message.
- The window holds open in the client core: the place of the main area, the thread of the side panel, and the pin list of the side panel.
- The side panel shows one thing: a thread, the pinned messages, or the members. Opening one closes the others. A thread and a pin list belong to the shown channel; when another channel or a direct conversation is shown, they are closed and do not open again by themselves.

Threads:

- "Reply in thread" in the actions and in the menu of a message opens the thread of that message, for an authored message of a channel that is no reply of a thread. The panel shows the root, the count of the replies, the replies and the composer. The replies start under the root; the list names a day only when it differs from the day of the root.
- A message with replies shows their count, the author and the time of the last reply, "Shown in panel" while its thread is open, and what the person has not read in the thread. The last reply is read from the open thread, or by the read of quoted messages of ADR 0740.
- The channel column lists under a channel its threads with something unread, by the start of the text of their root, and the thread of the side panel.
- The read position of a thread follows what the panel shows, by the rule of ADR 0740 for a list.
- "Also send to the channel" is not drawn.
- The chat of the call of this device offers no thread and no pin list: its side panel is the panel of the call. A message there shows the count of its replies. The channel column lists the unread threads of that voice channel as rows that take no click and tell the reason.
- A place whose history the server does not give shows the messages that wait to be sent under its statement, so that the person can send them again or discard them. This holds for a thread that is gone as well.

Fork:

- The panel offers the fork into a text channel and into a voice channel. The roles of the person predict the permission; without "manage channel" the two controls are disabled and say why. The server decides.
- A dialog takes the name of the new channel. It stays while the request is on its way, and states a refusal in words: no permission, the thread has a fork already, the name is refused, the thread is gone, and every other answer by the sentence of its error code. A changed name ends the statement.
- After the server made the channel, the dialog closes, and the panel names the fork channels in place of the two controls. A press on one shows that channel.

Pins:

- "Pins" in the header of a channel opens the pin list in the side panel: each pinned message with "Go to message" and, with "manage messages", "Unpin". "Go to message" goes to the message as a notice does (ADR 0742): the list of its place, or the thread of a pinned reply, reads the messages around its position when it does not hold the message, shows it and marks it.
- "Pin" and "Unpin" are in the actions and in the menu of a message for a person whose roles hold "manage messages". A refusal is told above the composer: the limit of 100 pins, a missing permission, and every other answer by its error code.
- A pinned message carries the mark "Pinned" without a name.

Permanent messages (ADR 0402):

- The header of a channel with permanent messages carries a marker whose explanation opens on hover and on focus. The composer states that a sent message cannot be edited, also the composer of a thread of that channel. The actions of a permanent message state "No edit or delete here"; edit and delete are not offered to its author.

Direct conversations:

- The channel column lists the direct conversations of the server in a section after the channels of the workspace, by the name of the other person. A conversation is shown in place of a channel, with the same list and composer. Selecting a channel ends that.
- The members panel offers "Message" for every other member. The conversation is opened on the server and shown when the state holds it, unless the person selected a server, a workspace, a channel, a conversation or the call after the request. The control of that member takes no click while the request is on its way. The refusal of the contact rule (ADR 0307) is stated at the member.
- A conversation whose two accounts may not write to each other any more says so in place of its messages.
- The name of a person who is no member of the shown workspace is read from the profile route.
- A message of a direct conversation offers no thread and no pin.
- The section of the direct conversations is drawn for every server that shows its lists, also without a conversation, and starts with "New direct message", before the conversations, so that a conversation that arrives does not move it. It opens a dialog with a field for a part of a name and the accounts that the server lists for it (ADR 0307), also those that are in no member list that the person can open. Choosing an account opens the conversation with it, as the members panel does, and closes the dialog.
- The dialog reads one page of 50 accounts at a time and "Show more" reads the next one, at most 500 accounts. "Show more" stands under the list at a place of its own and is drawn in every state, disabled while there is nothing more or a read is on its way: the rows of its answer join the list after the rows before, so neither the button nor a row moves under the pointer; a person who has not found an account then types more of its name. One read runs at a time: a part that the person types during a read is read after it, and the answer of a part that the person no longer wants is not shown. A read that failed is stated with "Try again".
- The list has a fixed height, and the state of a read is drawn in a slot of fixed height above it, so an answer moves neither the field nor the buttons of the dialog (ADR 0730). A row is an element named by its account, so a press that starts on one account and ends on another after an answer chooses nobody.
- The dialog belongs to the account that it was opened for: it closes when the server shows another account or none.
- Test: `contacts::a_person_writes_to_an_account_that_is_in_no_member_list_of_the_person` writes from a member to an administrator of a workspace of her own, whose name the window shows, sees that another member of that workspace is not offered, and as an administrator then finds that member; "Show more" keeps its bounds when an answer brings fewer rows. The test failed before the channel column had the control, and before "Show more" had a place of its own.

Unread signs:

- The sign of a channel, of a workspace and of a server follows the fields of ADR 0409: the dot when the level of the place wants what is unread, and the mention count that the settings let through. A muted place shows no sign, and its name is drawn lighter.
- "Mark as read" is offered while something is unread, with a sign or without one.

## Consequences

- A person sees one avatar on a message with a thread, the one of the last reply, where the design draws three.
- With the default level "mentions only", a channel with new messages and no mention has no dot. The person changes the level to see one.
- The channel column lists no thread that is read, except the open one: the server lists only threads with something unread.
- The menu of a message opens at the start of its text. Before this record it opened at the left edge of the list and lay over the channel column.
- A person in a call reads the threads of the chat of that voice channel after the call.
- Window tests against the real server cover a reply in the panel with the count and the unread sign, the refusal of a fork for a missing permission, for a name and for a second fork, and its success, a pin from the list and from the menu with its refusal, a direct conversation from a member, and the marker of permanent messages.
- The notice list, the levels and quiet times and the system notifications are in ADR 0742, the search panel in ADR 0743, and the card of a recording in ADR 0744.
