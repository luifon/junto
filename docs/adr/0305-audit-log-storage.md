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
Ban and server removal entries add optional `reason`; older entries read it as absent. The workspace action `workspace_member_removed` names the affected account and optional reason. These additive fields preserve existing names, types and meanings. Reasons have the same readers and immutability as their audit entries (ADR 0106).

- The entry of a revoked invite states that account in `invite_created_by`. The first version of `invite_revoked` named the invite only, so an entry that was written before has no such field.

Read-only:

- A trigger refuses every update, deletion and truncation of the table.
- The API has no route that changes an entry.

Scope of each action:

- About the server: owner claim, owner assignment, issued owner codes and owner return by host command (ADR 0114), invites and their redemption (ADR 0115), joins, approvals and denials of join requests, removals, bans, unbans, recoveries, server roles, creating and renaming a workspace.
- About a workspace: workspace roles and their assignment, categories, an account added by an administrator (ADR 0115), and removal from a workspace (ADR 0106).
- About a channel: creating, changing and deleting the channel, and its overrides.

Reading:

- `GET /api/v1/audit-log` returns the entries about the server and needs "view the server audit log".
- `GET /api/v1/workspaces/{id}/audit-log` returns the entries of one workspace and needs "view the workspace's audit entries".
- In the workspace log, an entry about a channel is returned only when the permission function allows the caller both to see that channel and to view audit entries in it.
- An entry about a deleted channel is returned only to a caller with the administrator permission (ADR 0407).
- The server decides once per channel and request, and the database selects the entries of the allowed channels, so a page is complete.
- The channels, the roles, the overrides and the entries of one request are read in one read-only transaction, from one snapshot (ADR 0009). A deletion that commits during the request does not change what the request returns.
- Entries without a time filter are ordered by identifier, newest first (ADR 0205). A time filter orders by occurrence time and then identifier, both descending, so the index serves bounded pages even when clock time and identifier order differ.

Filters:

- Both lists accept an exact `action`, `actor_id`, `target_id`, inclusive RFC 3339 `since` and exclusive RFC 3339 `until`. Filters intersect; they never widen the audience. An unknown action matches nothing. An action has 1 to 64 lowercase ASCII letters or underscores; an invalid interval is refused.
- A target is an identifier explicitly named by the event, including lists of identifiers and its workspace, channel and call scope. The actor is matched by `actor_id`, not implicitly by `target_id`. Text such as a reason is never interpreted as an identifier.
- `before` is the last returned identifier. With a time filter its occurrence time and identifier are the continuation pair; the anchor must still pass the same filters and audience, or the page is empty. The caller repeats the same filters for the next page. The query filters before taking the page, so an invisible or nonmatching entry does not make a page incomplete. Each page returns at most 200 entries and no total count.
- B-tree indexes select action, actor and time within the scope. A separate immutable table indexes each distinct target and entry identifier. A trigger fills it in the entry transaction; migration fills it for older entries. Filters bind values and add only the predicates in use, so optional parameters do not prevent indexed plans.

## Consequences

- No person can change or delete an entry through the application, and a statement that tries it in the database fails. A person who can drop the trigger can.
- A change and its entry are stored together or not at all.
- A viewer of the workspace log does not learn the name, the identifier or the existence of a channel that the viewer cannot see.
- A person who could see a channel loses its entries when the channel is deleted.
- A holder of "view the server audit log" does not see workspace entries without the workspace permission.
- The order follows the clock of the server at the time of writing.
- System lines in the channel (ADR 0303) are added with messages.

Audit reads share a limit of 120 requests per minute per account, including access tokens. Action and actor together have a composite index; time queries have matching scope/action/actor/time indexes. These indexes add startup migration and insert cost to avoid sorting or walking the full log for each page. Target extraction accepts only canonical UUID strings under identifier keys or identifier-list keys; other strings and reason text are not targets. UUID generation time does not bound occurrence time, so no time filter is inferred from identifier bits.
