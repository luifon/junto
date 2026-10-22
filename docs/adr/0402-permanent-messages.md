# 0402. Permanent messages

## Status

Accepted

## Context

Some channels hold records that must not change after they are written. Normal messages can be edited at any time without an edit history and can be deleted (ADR 0401). A setting that exists only on the channel would let a person with "manage channel" turn it off, change the messages and turn it on again. Retention and automatic deletion after archiving (ADR 0406) would also remove such records.

## Decision

- Permanent messages are a channel setting, off by default.
- Permanence is stored on each message. A message sent while the setting is on stays permanent after the setting is turned off.
- Nobody can edit a permanent message. This limit is checked before the administrator permission (ADR 0301).
- Only administrators can delete a permanent message. Each deletion leaves a line in the channel and an audit entry (ADR 0303).
- Changing the setting needs "manage channel". Turning it on records the audit entry `permanent_messages_turned_on` and turning it off records `permanent_messages_turned_off`, each with its system line in the channel, which the window shows as "turned on permanent messages" or "turned off permanent messages". A request that names only this setting records no `channel_updated` entry; a request that names it with nothing else and does not change it records nothing.
- File retention skips the attachments of permanent messages.
- Automatic deletion after archiving skips a channel that contains permanent messages.
- A channel that contains permanent messages can be archived.
- Only the server owner can delete a channel that contains permanent messages. The deletion is an explicit action with an audit entry, and the screen offers an export before it.

## Consequences

- Turning the setting off does not make earlier messages editable.
- An author cannot correct an error in a permanent message. The correction is a new message.
- An author cannot delete their own permanent message; an administrator can, and the deletion is visible in the channel.
- A channel that contains at least one permanent message is kept after archiving until the server owner deletes it, so its files, recordings and transcripts continue to use disk space.
- An administrator who is not the server owner cannot delete such a channel.
