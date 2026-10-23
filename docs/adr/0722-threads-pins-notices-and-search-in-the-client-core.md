# 0722. Threads, pins, notices and search in the client core

## Status

Accepted

## Context

ADR 0409 defines the routes of threads, forks, pins, notices, notification settings and search. Its events carry less than a client needs. `message_changed` names a channel, a message and a position, and not the thread of a reply; a reply creation or deletion sends a second `message_changed` for the root. The state of a thread names its root and no channel, and exists only for an account that wrote in the thread or was mentioned in it. A notice names a message and a kind, without text and without a place. `notification_settings_changed` names nothing. A read position marks notices as read on the server and sends no event of the notices. The server counts the thread routes, the list of unread threads, the pin list, the notices, the settings reads and the search against one limit of 120 requests in a minute for an account, shared with the history pages; `GET /messages/{id}` and the summary of a place are not counted. ADR 0719 holds one state per server with one writer, and ADR 0721 adds the timelines, the reads that events ask for and the waiting messages.

## Decision

Routes:

- The API object of a server has one function for each route of ADR 0409 that a member uses: the page of a thread, a reply, a fork, the read position of a thread, the unread threads with their continuation, the pin and the pin list, the notices and their read mark, the notification settings of a scope, the default level of a workspace, and the search with its filters and its continuation.
- A scope of notification settings is the account, a workspace or a channel. The account is written as the nil identifier (ADR 0409).

A thread is a place:

- The type that names a place (ADR 0721) gets a third kind: the thread under one message of a channel. A thread reuses the timeline, the pages, the anchor, the waiting messages and the read position of a place. Its timeline also holds the root and the fork channels, which each page of the thread carries.
- The file of a reply is declared for the channel of the thread. A waiting reply is confirmed only by a reply of the same thread.
- A server is not trusted to answer for what was asked (ADR 0714). The root of a thread page must be the message that the thread was opened for, in the channel of the thread. A message of another place in a page or in a pin list is not taken, and the timeline of that page says that its read failed. A page that adds no message does not promise more messages in its direction: the next read would start at the same message. An answer with another message than the one that was asked for is not used, and the message is not asked for again: a named message is then marked as failed, and for another message the timelines that can hold it are marked as not current.
- One function puts a message that the server returned where the state holds it: a reply into the timeline of its thread and never into the timeline of its channel; another message into the timeline of its place, into the root of an open thread, into the open pin list of its channel, and into the named messages. One function removes a message that the server no longer has from the same holders. A thread whose root is gone holds nothing and says so.
- `message_changed` asks for the message when a holder wants it: the timeline of the place, an open thread of the channel, the open pin list of the channel, or a list that names the message. The read is the uncounted `GET /messages/{id}`.
- The fork channels of an open thread are read again, with a page of one reply, after `channel_created` and `channel_deleted`: a fork is a new channel, and the deletion of a fork channel frees the thread for another fork. That answer changes the fork channels and not a root that the timeline holds, which follows the events of its message.
- A read of one message that fails for good marks every timeline that can hold the message as not current: the one of its place, and the open threads of its channel.
- A place whose page the server answers with `not_found` holds no message any more, and says so. This is the state of a direct conversation whose two accounts may not write to each other any more (ADR 0307). A page before or after the loaded messages starts at a message, and the server answers `not_found` also when only that message is gone. The client then reads the messages around the position of that message, which names no message, and that answer decides.

Unread threads:

- The state holds the list of the snapshot. It is read again from its route after a `message_changed` of a channel whose position is not above the highest position that the client knows for that place, after `thread_read` when something stays unread or the list does not hold the thread, after `message_mentioned`, and after `notification_changed`.
- The position rule tells a new message of a channel from every other change without a request. The notice of a root follows the notice of its reply, and a root is older than its reply, so the notice of a root never has a new position. The client keeps the highest position that an event named, beside the one of the summary, so that a summary that is older than an event does not lower it. An edit of the newest message also asks for the list: the edit can add a mention in a thread.
- Two reads of the list are at least 2 seconds apart. The value is part of the stream policy.
- `thread_read` moves the read position of a held thread. A thread that is read leaves the list. The answer to a read position that this client sent is not applied when the list holds a later reply of that thread or a later read position of it: the list is read. The event names no channel, and an unread mention of a thread counts in the summary of its channel, so the summary of each channel that shows mentions is read.
- A reader asks for the page after the loaded ones. A new first page replaces the list. The state holds at most 2,000 threads.

Named messages:

- The roots of the unread threads and the messages of the notices are read with `GET /messages/{id}` and kept by identifier, because a reader needs their place and their text. They share the bound of 4 single-message reads of ADR 0721. An event that names one reads it again; `not_found` marks it as gone; a read that failed for good is marked and tried again after a reload.
- A message that no list names any more leaves the state. A deleted channel removes the entries whose message is known to lie in it.

Pins:

- A reader opens and releases the pin list of a channel, as the members of a workspace (ADR 0719). The list is read once. After that it follows the single-message reads: a returned message with the pin is replaced or added at the end, where the server lists the newest pin, and a message without the pin or a deleted one leaves. The counted pin route is therefore not read for each change of a channel.
- A reload reads the list again. A list whose channel the new state does not hold is closed.
- A pin, an unpin and a fork are requests of a reader. The state changes when the event of the server was followed (ADR 0739).

Notices and settings:

- The state holds the notices of the snapshot and the notification settings of the account. `notification_changed` reads the first page of the notices again. Two reads of the first page are at least 2 seconds apart, as two reads of the unread threads are; the events during that time make one read at its end. A mention in each of many messages would otherwise make one read for each, and with the reads of the unread threads they pass the limit of the server for reads (120 in a minute, shared with the history). A reader asks for the page after the last notice; that page is added only while that notice is still the last one. The state holds at most 500 notices, the number that the server keeps.
- `message_read` and `thread_read` read the notices again when an unread notice can be marked by that position: its message lies in that place at or before the position, or its message was not read yet.
- `message_read` at the latest position of a place sets its unread count to zero. Its sign ends without a read of the summary only when the summary showed no mention: a mention can be one of a thread, which the read position of the channel does not reach. This narrows the rule of ADR 0721.
- `notification_settings_changed` reloads the state, as `call_access_ended` does (ADR 0719). A setting changes the unread sign, the mute and the mention count of every summary in its scope, and only the snapshot lists every setting.
- The summary of a channel is read 2 seconds after the stated end of its mute, and after that end again every 30 seconds while it still says "muted". Before that end no read is added. The server decides the end by its clock when it is asked and sends no event, and the two clocks can differ.
- The state answers whether the account is in its quiet time: "do not disturb", or the daily time with the rule of the server for its two minutes and its distance from UTC.

Search and the jump:

- A search is a request of a reader and holds no state in the model. Each page is decided by the access of its moment (ADR 0409), so a reader runs a search again after a reload.
- A reader shows one message of the history with a new read of an open place: the 30 messages up to a position and the 100 after it replace what is loaded, and the position becomes the anchor of the place for a reload. The page reads of that place that were on their way end.

Tool:

- `junto-cli` has `threads` (`list`, `reply`, `state`, `read`, `unread`, `fork`), `pins` (`list`, `add`, `remove`), `notices` (`list`, `read`, `settings`, `set-settings`, `default`, `set-default`) and `search`. `set-settings` reads the stored settings of a scope, changes the named ones and sends the whole: the server replaces the settings of a scope. `state` prints the unread threads, the notices and the settings, and `watch` the four new events.

## Consequences

- A reply that the server tells costs the client the read of its message when a thread or the timeline of its channel is open, and one read of the unread threads within 2 seconds.
- A channel with many new messages and no thread activity costs no thread request.
- The unread threads of a server at the background level are as old as its last snapshot or its last mention, because that level delivers no change of a channel (ADR 0107).
- A change of a notification setting on one device reloads the state on every device of the account. Open timelines stay visible during that reload (ADR 0721).
- The client cannot name who pinned a message, and knows of a thread only the count of its replies and its last reply, because the server tells no more.
- A field for the thread root in `message_changed` would replace the position rule and the spaced read.
- `uuid` is a direct dependency of the client core: the protocol names notices and scopes of settings with its type.
- The model is tested without a server for each new event and rule; the stream client against played routes for the thread as a place, the spaced read, the pin list, the notices, the reload after a settings event, the end of a mute and the jump; and two clients against the real server for a thread with its unread state, a mention with its notice, a pin, a fork, a search that finds a message and a transcript line, a direct conversation, and the loss of a channel.
