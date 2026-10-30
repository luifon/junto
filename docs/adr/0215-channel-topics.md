# 0215. Channel topics

## Status

Accepted

## Context

A channel name identifies its place in the list but does not state its purpose. The purpose must follow the channel through the API, event stream and transfer of a server structure.

## Decision

- Text and voice channels have a plain text topic, initially empty, stored in `channels.topic` by migration 0049. It has at most 1024 Unicode characters. Outer whitespace is removed, carriage returns are normalised to line feeds, line feeds and tabs are allowed, and other control characters are refused. The database also bounds its length.
- Creation accepts an optional topic. A patch without `topic` keeps it; an empty topic clears it. The existing manage-channel permission authorises changes. An archived channel takes no change. Channel responses, discovery and channel events carry the value; a reader of an older response treats its absent topic as empty. Channel update audit records include the topic, with empty as the default for older records. Event revision 13 records the added channel field.
- Structure format 5 exports topics and reads formats 1 to 4. An absent topic keeps an existing value, while an empty string clears it. Creation and update changes carry topics through the same permission and validation functions as the single routes. A plan of an export against the same server changes nothing.
- The channel header shows a topic below the name on one line, with its full text in a tooltip. The text shrinks within the header at narrow widths. Its shared context menu copies the complete value and, for an account that may manage an active channel, opens its topic editor. Actions check the captured owner and channel.
- `/api/server` advertises `capabilities.channel_topics`. Absent capabilities or an absent flag mean unsupported, including in older saved server entries; each contact refreshes the advertisement and persists both newly supported and withdrawn capabilities in the server book, so a restart uses the last confirmed advertisement. The client offers the topic editor only when the current saved server advertises it and refuses unsupported writes before sending them. A successful channel patch response must explicitly contain a string topic when the request changes it; an older success body cannot be mistaken for a saved topic. Ordinary name/category updates and older channel reads retain their defaults. Mixed-version window tests cover a missing capability and a successful response without a topic.
- The channel overview has a multiline editor, including an empty value. It uses the shared input editing menu, existing mutation acknowledgements and refusal handling. Other viewers receive the changed header through the event stream.

## Consequences

- A member can read and copy a channel's purpose without opening administration.
- The purpose is kept through restart and export/import rather than in the local window state.
- Tests cover create/update/read, permissions, archive and length/control limits, setup changes and keep/clear/create plans, two windows editing and copying, multiline text and the narrow header.
