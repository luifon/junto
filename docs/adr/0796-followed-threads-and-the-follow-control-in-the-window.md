# 0796. Followed threads in the channel column and the follow control of a thread

## Status

Accepted

## Context

An account follows a thread by a rule of the server or by its own choice, and the server lists the threads of a channel that the account follows (ADR 0412, ADR 0413). The client core has the routes and applies `thread_follow_changed` to the unread threads (ADR 0792). The channel column lists under a channel the threads with something unread and the open thread, and the side panel shows one thread (ADR 0741). The state of a server holds the follow state of a thread only while the thread has something unread. The approved design shows threads under a channel and has no control for the choice.

## Decision

The control in the thread panel:

- Under the head of the thread, one line states whether the account follows the thread and why: by its choice, by the rule of the server, not by its choice, or not at all. One action stores the other choice: "Follow" or "Stop following". While a choice exists, a second action "Use the default" removes it.
- The window reads the state of the shown thread from the route of the thread. An answer `not_found` means that the account does not follow the thread and made no choice. Until the state is read the line is absent.
- `thread_follow_changed` for the shown thread sets the state without a read. After a reload of the state of the server and after missed notices the state is read again; what was read before stays shown until the answer.
- One change at a time: the actions are disabled while a request is on its way. When an event tells the state during a request, the answer of the request does not replace it: the answer can be older than the event, so the state of the event stays and the state is read again. Such an answer does not change the list of the channel either; the list is read again. A refusal is one sentence under the line, until the next request or another thread. An archived channel takes no change, and the line says so.
- "Use the default" removes the choice and then reads the state, because only the server knows what its rule gives for the account.

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
- The control and the row that offers further threads are designed in this record and not in the approved design.
