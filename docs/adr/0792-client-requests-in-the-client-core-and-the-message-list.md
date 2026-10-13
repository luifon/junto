# 0792. The client requests in the client core, the authors of a thread and the lines of a direct call

## Status

Accepted

## Context

The server added what earlier client units asked for (ADR 0412, ADR 0413, ADR 0518): one request that marks every notice read with the event `notifications_read`; the choice to follow a thread with `thread_follow_changed` and two fields of the state of a thread; the followed threads of a channel; a notification default of a workspace that can be absent, with its removal and the event `notification_default_changed`; the daily quiet time by the name of a time zone; the first authors of the replies on a root message; and system lines of a direct call in the conversation of its two accounts, with a notice and an unread count for a call that was not answered. The state of a server in the client holds the notices, the unread threads and the notification settings (ADR 0722). The window draws a thread line under a message with replies and system lines in the message list (ADR 0740, ADR 0741). The client core has no time zone data.

## Decision

Routes of the client core:

- One function for each new route: every notice read up to a time, the choice to follow a thread and its removal, the followed threads of a channel in pages, and the removal of the notification default of a workspace. The read of the default returns a level or none.
- `junto-cli` has an action for each of them, prints the choice of a thread with its state, and takes the quiet time with `--time-zone`.

Events:

- `notifications_read` marks every unread notice that the state holds and that was created at or before the stated time. The first page of the notices is read again: a notice of that time whose transaction ended after the request is unread on the server (ADR 0412), and the list of the server decides.
- A page of notices that was asked for before the event and arrives after it is marked by the same event after its answer.
- `thread_follow_changed` sets the two fields on the thread when the list of unread threads holds it, and that list is read again: the server decides what is unread in a thread that is no longer followed.
- `notification_default_changed` sets the level of the workspace in the state, and the state is replaced by a snapshot. The level decides the unread sign of every channel of the workspace that sets none of its own, and only the snapshot states them all. The event `notification_settings_changed` does the same for the own settings of the account.

The quiet time:

- The function that tells whether the account is in its quiet time takes, from its caller, the minute of the day that the clock of a named zone shows now. A zone that the caller does not know gives no quiet time, as an unknown zone does on the server (ADR 0412).

The thread line of a message:

- The line shows the pictures of the first authors of the replies that the server states, at most three, small and overlapping, as the approved design draws them. A longer list of a server is cut at three.
- A root without stated authors shows the picture of the author of the last reply, as before.

The lines of a direct call:

- The four actions of ADR 0518 are system lines of the message list of a direct conversation: who started the call, who did not answer whose call, who declined, and that the call ended, with its length in hours and minutes, minutes and seconds, or seconds. A line without a length states the end only.
- The conversation of a call that was not answered has the unread count of the server, as a message has. The window adds no sign of its own.

Not built in this record:

- The followed threads of a channel in the channel column, and a control for the choice in the thread panel. The routes and the event are in the client core.
- The window parts of "mark all as read", of a default that is not set, of the quiet time with a list of zones, of a search result that opens at its position, and of the lines of a transcript around a line. Their screens are not in this branch of the client.

## Consequences

- A device shows every notice as read after one request of another device of the account.
- A change of the default of a workspace costs each client that sees the workspace one snapshot request.
- The client core needs no time zone data. The window that draws the settings of the quiet time gives the clock of a zone.
- A person who starts the application after a call that was not answered reads the call in the conversation and sees its unread count.
- The lines that a window drew for a direct call from live events are replaced by the stored lines where both exist.
