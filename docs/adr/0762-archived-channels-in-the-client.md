# 0762. Archived channels in the client

## Status

Accepted

## Context

An archived channel is in no channel list and not in the snapshot, and a person who sees it reads all of it (ADR 0410). `channel_archived` carries the channel with the time of its archiving and the time before which the server does not delete it. The state of a server in the client finds the place of messages through the channel list (ADR 0721), so a channel that leaves the list would lose its open messages, and a person who reads a channel at the moment of its archiving would see it vanish. After a reload the snapshot does not name the channel.

## Decision

Client core:

- The state finds a channel for reading in the channel list or among the archived channels that it knows. The messages, the threads and the pin list of a known archived channel stay readable.
- The state knows an archived channel from the loaded archived list of its workspace (ADR 0760). Beside the lists it keeps a channel that was archived while a reader had its messages, a thread of it or its pin list open, and a channel that it carried over a reload for such a reader.
- `channel_archived` for a channel that no reader has open and whose archived list is not loaded leaves nothing of the channel in the state except its notices with their messages: the server still lists them (ADR 0410), and a reload carries them in the snapshot. A deleted channel leaves no notice, and a deleted workspace leaves no notice of any of its channels, also of an archived one that the state did not know.
- When the archived list of a workspace is released or its read fails, the channels of it whose messages a reader has open are kept beside the lists.
- After a reload the state carries an archived channel whose messages, a thread of it, its pin list or a collection of it (its overrides, the permissions of the account in it) a reader has open, when the snapshot holds its workspace and not the channel. When the archived list of that workspace answers, the events of the time of its read are applied first, and then the list decides: the messages, the pins and the collections of a channel that is in no list are closed.
- When no reader holds the archived list of the workspace of a carried channel, the client reads the list for that check alone, and lets go of it after the answer: the carried channels that the list holds stay while a reader holds them. A check that fails leaves the carried channels unchecked until the next reload. Reason: the client core checked a carried channel only when a reader held the list, and a client that held the messages or a collection of the channel without the list kept a channel that the server had deleted until the next reload, and its collection stayed open. Test: `a_carried_archived_channel_is_checked_without_a_reader_of_its_list` in `crates/client-core`.
- `channel_deletion_due` replaces a channel that the state knows as archived. `channel_restored` and `channel_deleted` remove it from the archived channels.

Window:

- The main area shows an archived channel while the person has it selected and the state knows it. The channel is in no section of the channel column. Its header has the mark "Archived". In place of the composer a bar states that the channel is archived and that nobody can write in it, when it was archived, when the server deletes it at the earliest, and "Restore" for a person whose roles hold "manage channel".
- The messages of an archived channel offer no reaction, no answer, no edit, no deletion, no pin and no new thread, in the main area, in the thread panel and in the pin list: the server refuses each of them (ADR 0410). A message of an archived channel has no menu and no bar of actions; its reactions and its check boxes are drawn without a click. A thread of an archived channel offers no fork.
- While the main area shows an archived channel, the window holds the archived list of its workspace open, so that a reload finds the channel again.
- Notices and search results name content of an archived channel (ADR 0410). While the list of the notices or the search is open, the window holds the archived list of each workspace that a notice or a result names with a channel that is in no channel list (ADR 0742, ADR 0743). A result names its workspace (ADR 0413); a notice takes it from its message. The window does not decide from the state whether the channel is archived: a channel that is in no channel list is enough, so a list that answers without the channel is held as well and is not read again. A workspace that the state does not hold has no list to wait for. A list that failed is stated in the panel with a retry.
- The page "Archive and retention" of the administration lists the archived channels of the shown workspace with "Open", "Restore" and the settings of the channel.
- The settings of a channel hold the archived list of its workspace open, so that the settings of an archived channel find it.

## Consequences

- A person who reads a channel while another person archives it keeps the messages on screen and is told that the channel is archived.
- A person opens an archived channel from the administration only. The channel column has no entry for it.
- Between a reload and the answer of the archived list the client shows an archived channel that the server may have deleted meanwhile. A read of its messages then fails, and the list closes it.
- A reload while a reader holds an archived channel without its list costs one read of the archived list of its workspace.
- A notice or a result of an archived channel costs one read of the archived list of its workspace while the list of the notices or the search is open.
