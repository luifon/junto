# 0410. Archived channels: routes, what stays allowed, the period, the notice and the deletion

## Status

Accepted

## Context

ADR 0406 states that archiving is manual, that an archived channel is read-only and leaves the channel list, that it is deleted a set time after archiving (30 days by default, adjustable per workspace, including never), that people with access get a notice 7 days before, and that automatic deletion skips a channel with permanent messages. It names no route, no permission, no rule for a call or for the settings of an archived channel, and does not state what a notice is or what happens when the period changes after the archiving.

## Decision

Routes:

- `PUT /api/v1/channels/{channel_id}/archive` archives and `DELETE` on the same path restores. Both need `manage_channel` in the channel. A channel that is already in the asked state stays as it is, without an event and without an audit entry.
- `GET /api/v1/workspaces/{workspace_id}/archived-channels` returns the archived channels that the caller sees, each with the time of its archiving and the time before which it is not deleted.
- `GET` and `PUT /api/v1/workspaces/{workspace_id}/archive-period` read and set the period: 7 to 36500 days, or `null` for never. They need the administrator permission. A workspace without a setting has 30 days.

What an archived channel allows:

- It is in no channel list and not in the snapshot. A person who sees the channel reads everything of it: history, threads, pins, search, attachments, recordings, transcripts and its audit entries. Read positions and personal notification settings still change.
- Every change of its content and of its settings gets `channel_archived` (409), a code of its own so that a client tells the archive apart from a value that changed meanwhile, which is `conflict`: sending, editing and deleting a message, reactions, pins, thread replies, forks, new uploads, the bytes and the completion of an upload that was started before, every route of its call, the start and the removal of a recording, corrections, speaker names and requests of a transcript, the channel settings and the call settings. The owner of an unfinished upload still reads its state and gives it up.
- Its overrides can be changed, it can be exported (ADR 0411), restored and deleted. Deletion follows the rule of every channel (ADR 0407).
- The call of an archived voice channel ends at the next admission check: nobody may be at the place (ADR 0509). A recording ends with its call.
- A restored channel is a channel of the list again and takes every change.

Events and audit:

- `channel_archived` and `channel_restored` go to the people who see the channel. The audit entries `channel_archived` and `channel_restored` are about the channel, so each appears as a system line in it. A change of the period records `archive_period_changed` about the workspace and the event `retention_changed`.

Notice and automatic deletion:

- Maintenance records one notice for an archived channel when its deletion is 7 days away or less: the event `channel_deletion_due`, with the time of the deletion, for the people who see the channel, and the system line `channel_deletion_due` in the channel. The time of the notice is stored on the channel.
- Maintenance deletes an archived channel 7 days after its notice, with the function of the manual deletion and no acting account. No channel is deleted without a notice that is 7 days old.
- The smallest period is 7 days, so that the notice of a channel is never later than its archiving.
- A change of the period removes the notices of the archived channels of the workspace. Each gets a new notice 7 days before its new time. A channel whose new period has ended already is deleted 7 days after that new notice.
- A restore removes the notice.
- A channel that contains permanent messages gets no notice and is not deleted by the server (ADR 0402). A workspace with the period "never" deletes nothing.
- Each transaction of maintenance records the notices of at most one batch of channels of one workspace, or deletes one channel. The statements select through partial indexes on the archived channels.

## Consequences

- An archived channel that must change is restored first.
- A person who opens the list of archived channels sees the time of each deletion at any time. A person who was not connected when the notice was recorded finds the system line in the channel.
- With the default period the deletion comes 30 days after the archiving, and its notice after 23 days.
- A shorter period never deletes a channel within 7 days of its change.
- The file that defines the structure does not state whether a channel is archived (ADR 0212).
- Search results, notifications and unread threads still name content of an archived channel.
