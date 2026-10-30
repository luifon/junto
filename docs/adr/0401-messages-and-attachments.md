# 0401. Messages and attachments

## Status

Accepted

## Context

Text channels, the text chat of each voice channel and direct messages carry messages with formatting and files. The rules for editing and deleting decide what the server must store and what other members can rely on. Attachments use disk space on the server, so their number and size need limits that the administrator of a server controls.

## Decision

Messages:

- A message can be a reply to another message.
- An author can edit their own message at any time. An edited message shows "edited". No edit history is kept.
- An author can delete their own message. Deleting another person's message needs "manage messages".
- A deleted message and its files are removed for everyone.

Mentions and reactions:

- A message can mention a person, a role, or everyone in the channel. Mentioning everyone in the channel needs the "mention everyone in a channel" permission.
- Reactions use standard emoji.

Formatting:

- Bold, italic, strikethrough, inline code, code blocks with syntax colours, quotes, lists, checkbox lists and links.
- Ticking a checkbox is an edit of the message, so only the author of the message can do it.

Attachments:

- A message has up to 10 files, and a file has up to 100 MB, by default. The server administrator can change both limits.
- An image shows a preview. Any other file shows as a row with its name and size.
- Uploads are sent in chunks and can resume (ADR 0006).

Pins:

- Each channel has pins.

Indicators:

- Presence indicators are absent. Typing indicators are transient signals for a text or voice channel chat, a direct conversation, or one thread. The signal carries an account identifier and display name, never draft content.
- Clients watch at most eight open message places over their current full event subscription. Actual composer edits announce activity at most once every two seconds. Clearing, sending, previewing, leaving the input or closing its window stops that composer. Restoring a saved draft announces nothing. Several local composers and devices of one account aggregate; stopping one source leaves other active sources visible.
- A signal lasts at most eight seconds. The server bounds active entries and revision keys to 8192, broadcasts to a queue of 256 signals, accepts at most eight places per connection, and rates changes to eight per account per second. Pending watch acknowledgements coalesce to the latest set. Disconnected connections remove only their own sources. Closing a connection and publishing its authorized activity use the same registry guard, so an interrupted read cannot revive a closed source.
- Sender and recipient sessions and place permissions are read for each delivery. Typing requires an open writable place; a new thread also needs Create threads. A recipient needs current place access and the thread root must satisfy the same live-history boundary as messages. Other accounts receive the signal only for watched places; the viewing account is excluded. Queued obsolete signals and signals whose lifetime elapsed are dropped.
- Typing activity has no database row, persistent event, cursor, snapshot field, audit record or replay. Reconnection clears the client display and watches again. Expiry bounds a lost stop signal. Limits bound memory and database work independently of message persistence.

## Consequences

- The earlier text of an edited message cannot be recovered or shown.
- A checkbox list in a message cannot be ticked by other members.
- A deleted file is removed from disk, and a finished export that contains a deleted message or file is deleted (ADR 0406).
- Messages in a channel with the permanent-message setting follow the stricter rules of ADR 0402.
- The server refuses new uploads while free disk space is below the reserve (ADR 0006).
- Custom emoji, link previews, inline video and audio players, and message buttons are in the later list of `docs/scope.md`.
- The format of message identifiers is not fixed by this record.
