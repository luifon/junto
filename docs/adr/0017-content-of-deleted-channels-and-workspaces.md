# 0017. The content of a deleted channel or workspace is removed in bounded transactions

## Status

Accepted

## Context

A deleted channel stays as a marker row, and its content is removed by the features that own it (ADR 0407). The deletion of a channel deleted every message of the channel in the transaction of the request. PostgreSQL ends a statement after 10 seconds (ADR 0009), so a channel with many messages could not be deleted at all, and the recordings of a deleted channel stayed with their files (ADR 0608). A workspace is deleted with every channel of it (ADR 0308), and an archived channel is deleted by the server (ADR 0410). Two foreign keys of `messages` had no index: each deleted message read the messages of its place, and each removed recording read every message.

## Decision

- A deletion changes, in the transaction of its request, only rows of the structure: it marks the channel as deleted, removes its overrides and its fork marker, and records its event and its audit entry. From that commit on, no route returns anything of the channel.
- The same transaction writes one row into the table `purges` for each deleted channel, and one for a deleted workspace. The row names what still has to be removed.
- One task removes the content of the subjects of `purges`, the oldest first. It runs after each deletion, once per minute, and as a part of maintenance.
- The task removes, for a channel: its recordings with their segments and transcripts, whose directories go into the queue of removals (ADR 0608); its exports (ADR 0411); its attachments, which the cleanup of ADR 0408 then deletes from the disk; its thread replies and then its other messages, with their reactions, pins, mentions and notifications; its read positions; the notification settings for the channel; its call notes; the private lines of its calls. For a workspace: the notification settings for the workspace.
- Each transaction of the task removes at most one maintenance batch of rows (500) of one table. Every statement selects its rows through an index, and a test reads each plan with sequential scans disabled.
- The rows that one message or one recording can have in numbers without a small limit are removed in batches of their own, before the row that they depend on: for each page of 500 messages, the thread positions, notifications, mentioned accounts, sent mention notices, mentions and reactions of those messages; the references of the messages that answer another message; the accounts that read the private lines of a call; for each recording, the names that people gave to its speakers, the corrections and the lines of its transcript. After that, the removal of a message removes with it at most its pin, its fork marker and the link of its attachments, and the removal of a recording removes its segments and the row of its transcript. A recording is removed in a transaction of its own.
- The migration that adds the table `purges` writes one row for each channel that was deleted before: those channels kept their recordings.
- A channel whose call has not ended, or that has a recording that is starting, runs or is processed, is left and taken again after 30 seconds. The admission check ends the call of a deleted channel (ADR 0509), and the recording ends with its call (ADR 0604).
- The row of `purges` is deleted in the transaction that finds nothing left. A failure leaves the row, and the next run continues.
- The task records no event and no audit entry for the single items. The owner of a removed export gets `export_removed`.
- The rows that refer to a deleted row are found through an index: `messages.reply_to`, `messages.recording_id`, `call_notes.recording_id`, the overrides of a role, the server roles of an account by role, the channels of a category, and the fork marker by its destination.
- The table of the private lines of a call stores the channel of the call, so that the lines of a deleted channel are found.

## Consequences

- The time of a deletion request does not grow with the content of the channel or of the workspace.
- The content of a deleted channel stays in the database and on the disk until the task has run. No route returns it in that time.
- The marker row of the channel, its row of message positions and its audit entries stay (ADR 0407).
- A deletion whose task is interrupted by a stop of the server continues after the start.
- A deleted channel whose recording never reaches a final state keeps its row of `purges`. The processing of recordings gives every recording a final state after three attempts (ADR 0606).
- The purge of a channel with a million messages takes two thousand transactions for the messages, and at least six further transactions for each page of 500 messages.
- While the rows of a message are removed and the message is not, the counts and the thread positions of that message are wrong. No route returns anything of a deleted channel.
