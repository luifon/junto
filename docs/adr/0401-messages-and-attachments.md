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

- There are no presence indicators and no typing indicators in the first version.

## Consequences

- The earlier text of an edited message cannot be recovered or shown.
- A checkbox list in a message cannot be ticked by other members.
- A deleted file is removed from disk, and a finished export that contains a deleted message or file is deleted (ADR 0406).
- Messages in a channel with the permanent-message setting follow the stricter rules of ADR 0402.
- The server refuses new uploads while free disk space is below the reserve (ADR 0006).
- Custom emoji, link previews, inline video and audio players, and message buttons are in the later list of `docs/scope.md`.
- The format of message identifiers is not fixed by this record.
