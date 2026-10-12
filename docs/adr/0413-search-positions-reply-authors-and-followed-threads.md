# 0413. Positions in search results, the first authors of a thread, and the followed threads of a channel

## Status

Accepted

## Context

ADR 0405 and ADR 0409 define the search and the threads. A search result named its message and not the position of the message, so a client read the message before it could open the history at that place, and did not know whether the message was a reply of a thread. A result for a transcript line named the line and not its time. The lines of a transcript were read from the first line or after a position, so a client could not open a long transcript at a line in its middle and read backwards. The root of a thread carried the number of its replies and its newest reply, and not who wrote in the thread. The list of threads of an account held the unread ones only (ADR 0409), so a client could not list the threads of a channel that the person follows and has read.

## Decision

Search results:

- A result for a message states `position`, the position of the message in its channel or conversation, and `thread_root`, the root of its thread when the message is a reply.
- A result for a transcript line states `start_ms`, the start of the line on the timeline of the recording.
- The fields come from the rows that the search reads already. The rules of ADR 0409 for what a search returns do not change.

Lines of a transcript around a line:

- `GET /api/v1/recordings/{id}/transcript/lines` takes at most one of `after`, `before` and `around`. `around` returns a page with the line at that position in its middle, and `before` the lines before a position, the nearest ones.
- The page states `next_after` when lines follow and `previous_before` when lines come before it, also for an empty page: a position beyond the last line states where the lines before it end, and a position before the first line states where the lines begin.
- The access is the access of the route (ADR 0604): the caller reads the recording.

The first authors of a thread:

- A root message carries `reply_authors`: at most three accounts that wrote a reply that is not deleted, in the order of their first reply.
- A table holds one row for each thread and author, with the position of the first reply and the number of replies of that author that are not deleted. The trigger that maintains the reply count of a root (ADR 0409) maintains it. An author leaves the list when their last reply is deleted. An author whose first reply is deleted and who has another reply keeps the place of the first one.
- A read takes the three rows of a root through an index of the root and the position.
- Whoever reads the root reads the list: the replies of a thread have the audience of their channel.
- A change of the replies records `message_changed` for the root, as before, and the client reads the root again.

The followed threads of a channel:

- `GET /api/v1/channels/{id}/threads` returns the threads of the channel that the caller follows (ADR 0412) and that have a reply, the one with the newest reply first. Threads without unread replies are included. Each entry is the state of the thread, as in the list of unread threads.
- A page has at most 200 threads, 50 without a stated limit, and a continuation that holds the newest reply and the root of its last entry.
- The caller sees the channel; an archived channel is read as its messages are (ADR 0410).
- One index of the account, the channel and the newest reply serves the list.

Not built:

- "Also send to the channel" stays in the later list (ADR 0403). A reply is in its thread only.

## Consequences

- A client opens the history at a search result without reading the message first, and opens the thread of a result that is a reply.
- A client shows the time of a transcript result and opens the transcript at that line.
- A client draws up to three authors on the row of a thread.
- A client lists the threads of a channel that the person follows, read or unread.
- A thread whose replies were all deleted is in no list.
