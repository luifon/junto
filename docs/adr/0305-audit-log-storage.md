# 0305. Storage and reading of the audit log

## Status

Accepted

## Context

ADR 0303 states what the audit log records, that it is read-only, and who sees an entry. The storage is part of the first build step, so that every later feature writes its entries from the start (ADR 0903). The record does not fix the layout of an entry, how the read-only rule is enforced, or how the two read permissions map to routes.

## Decision

Entry:

- An entry has an identifier, the time, the acting account, the workspace and the channel it is about, and the event.
- The acting account is absent when the server itself acted.
- An entry is about the server (no workspace), about a workspace (no channel) or about a channel.
- The event is one typed value per action, stored as JSON with its action name. The code has one variant per action with named fields.
- A new action is a new variant. The fields of an existing action are not changed, so stored entries stay readable.
- An entry is written in the transaction of the change that it describes.
- An entry refers to an invite by its identifier and states the account that created the invite (ADR 0105). It never contains a code, a token or a key.

Read-only:

- A trigger refuses every update, deletion and truncation of the table.
- The API has no route that changes an entry.

Scope of each action:

- About the server: owner claim, owner assignment, issued owner codes and owner return by host command (ADR 0114), invites and their redemption (ADR 0115), joins, approvals and denials of join requests, removals, bans, unbans, recoveries, server roles, creating and renaming a workspace.
- About a workspace: workspace roles and their assignment, categories, an account added by an administrator (ADR 0115).
- About a channel: creating, changing and deleting the channel, and its overrides.

Reading:

- `GET /api/v1/audit-log` returns the entries about the server and needs "view the server audit log".
- `GET /api/v1/workspaces/{id}/audit-log` returns the entries of one workspace and needs "view the workspace's audit entries".
- In the workspace log, an entry about a channel is returned only when the permission function allows the caller both to see that channel and to view audit entries in it.
- An entry about a deleted channel is returned only to a caller with the administrator permission (ADR 0407).
- The server decides once per channel and request, and the database selects the entries of the allowed channels, so a page is complete.
- The channels, the roles, the overrides and the entries of one request are read in one read-only transaction, from one snapshot (ADR 0009). A deletion that commits during the request does not change what the request returns.
- Entries are ordered by identifier, newest first (ADR 0205).

## Consequences

- No person can change or delete an entry through the application, and a statement that tries it in the database fails. A person who can drop the trigger can.
- A change and its entry are stored together or not at all.
- A viewer of the workspace log does not learn the name, the identifier or the existence of a channel that the viewer cannot see.
- A person who could see a channel loses its entries when the channel is deleted.
- A holder of "view the server audit log" does not see workspace entries without the workspace permission.
- The order follows the clock of the server at the time of writing.
- System lines in the channel (ADR 0303) are added with messages.
