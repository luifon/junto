# 0790. The permissions of the account in the client: the lists of the server

## Status

Accepted; replaces the permission parts of ADR 0719, ADR 0760 and ADR 0761.

## Context

The client offered an action when the roles of the account in the workspace gave its permission (ADR 0719). It did not know the overrides of a channel, which only a person with "manage channel" reads, so it offered an action that a deny override refuses and did not offer one that an allow override grants. For a person who read the overrides, the client core computed the permissions of a channel with the permission function of the server (ADR 0760). The snapshot now states the workspace permissions of the account for each workspace, and for each channel in which the account holds another list (ADR 0310). An archived channel is in no snapshot, and one route states the list of the caller in one channel. `permissions_changed` stays the one event for a change of what an account may do.

## Decision

The state:

- A workspace of the state holds `permissions`, the list of the workspace, and `channel_permissions`, one list for each channel that has another one. Both are taken from the snapshot as they are. An entry for a channel that the snapshot does not carry is dropped.
- The permissions of the account in a channel of the channel lists are the entry of the channel, or the list of the workspace when the channel has no entry.
- No event changes a list. An event that changes a role, the roles of the account or an override changes that object in the state, and the list changes with the snapshot that follows `permissions_changed`.
- A channel that leaves the channel lists, by a deletion or by archiving, takes its entry with it.
- A workspace that an event brings holds an empty list until its snapshot. The server records `permissions_changed` for the accounts that see a new workspace (ADR 0310).
- A channel that an event brings has the list of its workspace. A fork and a restore of a channel with overrides record `permissions_changed` (ADR 0310).

An archived channel:

- The permissions of the account in an archived channel are a collection that a reader opens and releases (ADR 0760), read from `GET /api/v1/channels/{channel_id}/permissions`. An answer that names another channel is refused.
- No event carries the list. It is read again after each snapshot, as every open collection is, and a change of it is `permissions_changed`.
- Until the collection is loaded the list is not known, and the client offers nothing that needs a permission in that channel.

What is removed:

- The function that summed the permissions of the roles of the account, and the function that applied the permission function of the server to the loaded overrides. The client compares no role and no override for a permission.
- The client core still depends on `junto-authz`, for role order only (ADR 0791).

The window:

- A control of one channel reads the list of that channel: the deletion and the pin of the message of another person, the fork of a thread, the gear and the pages of the settings of a channel, the restore and the export of a channel, the choices of the overrides page, `@everyone` in the composer, the recording control, the lock, meeting mode, and the menu of a person in a voice channel. A move of a person is offered into a voice channel in which the account holds "move members" too (ADR 0511).
- A control of a workspace reads the list of the workspace: the pages "Channels" and "Roles", the audit log of the workspace, a new channel and a new category, and the restore and the deletion in the list of archived channels.
- A row of the page "Channels" reads the list of its channel: "Move to…" needs "manage channel" in the channel, and a move by one place needs it in every channel whose position the move changes.
- The server permissions of the account are in the account of the snapshot, as before.
- The window holds open the permissions of an archived channel while the main area shows that channel and while its settings are open.

## Consequences

- A person who holds a permission by an override of one channel is offered its actions in that channel, and a person whose role is denied a permission in one channel is not.
- The window follows a change of a role, of an assignment and of an override with one snapshot request, without a rule of its own.
- Between an event that changes a role and the snapshot after `permissions_changed`, the window shows the role as changed and the controls as before.
- The list of archived channels offers restore and deletion by the list of the workspace. An override of one archived channel can differ; the server decides, and the settings of that channel read its own list.
- A state that a test or the screen tool builds by hand states its lists. A helper states them as a server without overrides does.
- The server still decides every request.
