# 0303. Audit log

## Status

Accepted

## Context

Administrators and moderators can change roles, change channel settings, move people, start recordings, make exports, admit and remove people, and delete permanent messages. Members need to be able to see that such an action happened and who did it. An entry about a private channel must not show the existence or the content of that channel to a person who cannot see it.

## Decision

- The server records these actions: role changes, channel settings, moves, recording start and stop, exports, invites, approvals, removals, bans, recoveries, and message deletions in permanent-message channels.
- Each entry states who did it, what was done and when.
- The log is read-only.
- A viewer sees an entry about a channel only if the viewer can see that channel.
- Entries about a deleted channel stay visible to server administrators.
- Actions that concern a channel also appear in that channel as system lines.
- The log records which invite code created each account (ADR 0105).
- Reading the server audit log needs the "view the server audit log" server permission. Reading the audit entries of a workspace needs the "view the workspace's audit entries" workspace permission (ADR 0301).

## Consequences

- No person, including an administrator and the server owner, can change or delete an entry through the application.
- The members of a channel see moderation actions on that channel in the channel itself, without access to the log.
- The visibility of an entry is evaluated per viewer with the permission function of ADR 0301.
- The storage of the audit log is part of the first build step (ADR 0903), so that every later feature writes its entries from the start.
- This record sets no retention rule for the log.
