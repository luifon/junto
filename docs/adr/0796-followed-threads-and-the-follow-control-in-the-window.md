# 0796. Followed threads in the channel column and the follow control of a thread

## Status

Accepted

## Context

An account follows a thread by a rule of the server or by its own choice, and the server lists the threads of a channel that the account follows (ADR 0412, ADR 0413). The client core has the routes and applies `thread_follow_changed` to the unread threads (ADR 0792). The channel column lists under a channel the threads with something unread and the open thread, and the side panel shows one thread (ADR 0741). The state of a server holds the follow state of a thread only while the thread has something unread.

## Decision

The control in the thread panel:

- The follow control is a bell in the header of the thread: a bell while the account follows the thread, a slashed bell while it does not, with the tooltip "Following" or "Follow". A press opens its small menu: "Follow" or "Stop following", with the sentence that states whether the account follows the thread and why (by its choice, by the rule of the server, not by its choice, or not at all) as its second line; and, while a choice exists, "Use the default" with the reason that it removes the choice. The menu of a thread row in the channel column has the same items with the same words. Reason: a state line and two buttons stood between the header and the root of every thread. Tests: `thread_actions::` and `threads::` of the window tests use the menu of the bell.
- The window reads the state of the shown thread from the route of the thread. An answer `not_found` means that the account does not follow the thread and made no choice. Until the state is read the line is absent.
- `thread_follow_changed` for the shown thread sets the state without a read. After a reload of the state of the server and after missed notices the state is read again; what was read before stays shown until the answer.
- One change at a time: the actions are disabled while a request is on its way. When an event tells the state during a request, the answer of the request does not replace it: the answer can be older than the event, so the state of the event stays and the state is read again. Such an answer does not change the list of the channel either; the list is read again. A refusal is one sentence under the header, until the next request or another thread. An archived channel takes no change, and the line says so.
- "Use the default" removes the choice and then reads the state, because only the server knows what its rule gives for the account.

The followed threads of the workspace:

- The entry "Threads" at the top of the channel column opens the side panel "Threads": the threads that the account follows in the channels of the shown workspace whose history it reads, the threads with unread replies first, each group with the newest reply first, each row with the start of the text of its root, its channel and its unread count. A press on a row shows the channel and opens the thread. The entry draws the count of the unread threads that the channel column lists. The list is the list of the followed threads of a channel (`FollowedList`) with the workspace in place of the channel: it is read by the same triggers, in pages of 50, and the panel offers the further pages. The server route `GET /api/v1/workspaces/{workspace_id}/threads` (ADR 0413) gives each thread with its channel. The panel lies under a thread, the pinned messages and a transcript, and over the activity and a search; another of them opening closes it. Reason: the followed threads of the other channels were not reachable from the channel column, and the column changed its length with each shown channel. The rows under each channel keep its unread threads and the open thread. Test: `threads::the_threads_entry_lists_the_followed_threads_of_the_workspace`.

The followed threads of the shown channel:

- The rows of the shown channel list, after the threads with something unread, the threads that the account follows and that have nothing unread, in the order of the server: the thread with the newest reply first. A thread is listed once. Other channels list their unread threads only, as before.
- At most 8 followed threads without something unread are listed. A last row offers the further ones; it lists everything that was read and reads the next page of 50. Another shown channel ends the longer list.
- The list is read when another channel is shown, after `thread_follow_changed`, when the unread threads of the state differ from the ones of the last read, when the count of the replies of the open thread changes, after a reload and after missed notices. One read is on its way at a time; a trigger during a read is followed by one more read. A read from the start asks for as many threads as are held, up to the largest page. A failed read keeps what is listed and draws no error.
- A thread that an event states as not followed leaves the list at once, and the answer of a read that was sent before the event is not used. When that read was the one of a further page, the page is asked for again after the list was read from its start, with the continuation of that read.
- A thread that a page names twice is listed once.

Titles:

- A row shows the start of the text of the root. The root is taken from the state of the server when it holds it: among the messages that its lists name, in the loaded messages of the channel, or as the root of the open thread. Otherwise the window reads the message, at most 4 at one time and each root once, and holds at most 200 roots. Only the first 200 rows without a root of the state get one: a longer list keeps the neutral title for the rest, so the reads end. `message_changed` for a held root drops it, and it is read again when a row needs it. A root that the server does not give has the neutral title.

Accounts:

- What is held belongs to the account of the session of its server. With another account it is dropped and read anew.

## Consequences

- A person follows a thread without writing in it, leaves a thread without leaving its channel, and returns to the rule of the server.
- A person finds a followed thread under its channel after its replies were read.
- The shown channel costs one read of its followed threads at each trigger, and one read for each root that the state does not hold.
- A reply of the own account in a thread that is not open, on another device, changes the order of the list only at the next trigger.
- The control and the row that offers further threads are designed in this record.
