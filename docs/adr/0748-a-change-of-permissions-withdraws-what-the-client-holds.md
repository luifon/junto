# 0748. A change of permissions withdraws what the client holds of its scope

## Status

Accepted

## Context

The server tells a change of what an account may see with the event `permissions_changed`, for one workspace or for the server (ADR 0207). The client replaces its state by a snapshot after it (ADR 0718). Until the snapshot is installed, the client held the state of before: the messages, the pins, the members, the unread summaries and the collections of the administration that the account may not see any more stayed on screen, and they stayed without a bound while the snapshot failed. A read that was sent before the change could still fill the state with an answer of before. The list of the recordings of a workspace is no part of the state (ADR 0784): it kept its rows after a change of permissions and after a deleted channel, and an answer of before could fill it again.

## Decision

The state of the client core:

- At `permissions_changed` the client withdraws from the state what the snapshot decides in the scope of the event: one workspace, or every workspace and the server. Until the snapshot is installed the state holds of that scope no message of a timeline or of a thread, no pin, no unread summary of a channel, no unread thread and no notice whose message the state holds and that lies in one of its channels, no named message there, no open recording, transcript or transcript line of its channels (or of a recording whose channel is not known yet), and no member. An unread thread and a notice whose message the state does not hold stay: they hold counts and identifiers, no text, and the snapshot replaces them. A timeline keeps its reader, its place and the position that the reader shows; it holds no message and is marked as being read. The pin list and the members are marked as being read.
- The collections of the administration of the scope are marked as being read: the overrides and the permissions of its channels, its archived channels and its settings, and for the server also the accounts, the invites, the join requests, the access tokens, the exports, the server roles and the retention of direct messages.
- The structure stays: the workspaces, the categories, the channels with their calls, and the roles. The selection, the open panels and a call keep their place, and the structure holds names that the account was shown already. The snapshot replaces it.
- The reads that were on their way at the event end, and their answers are not taken. The snapshot reads every part again, as after every snapshot.
- A timeline that is withdrawn takes no message from an event or a read until the page that the snapshot asks for arrives.

The window:

- The results of the search of the server leave at once, the answer of a search that started before is not taken, and the search is read again.
- The shown audit log of the scope, or of the server, holds nothing of before, takes no answer of a read that started before, and is read again from its start.

The list of the recordings in the window:

- `permissions_changed` for the workspace of the list or for the server, `channel_deleted` of a channel of that workspace, a replaced state, and steps of the event stream that the reader missed withdraw the list: it holds no row and states that it is being read. A read that had started before takes nothing of its answer. The read after it fills the list, at the pace of ADR 0784.

## Consequences

- A person who loses the access to a channel sees its messages, pins, members and unread signs no longer from the moment of the event, also when the snapshot fails.
- A person whose permissions change in a workspace sees its lists as being read for the time of one snapshot and one page, also when the change gives more than before. A further change before the state stayed for 30 seconds waits for its snapshot as ADR 0718 states (1 second, doubling up to 60 seconds), and the lists stay empty for that time.
- A failed snapshot leaves the scope empty and the status of the stream tells it (ADR 0719).
- Tests: the model withdraws a workspace and the server; the stream client against played routes holds a page answer of before at the gate while the snapshots fail; the list of recordings withdraws for each event that concerns it, and takes the answer of a read after the change only.
