# 0310. The permissions of the account in the snapshot

## Status

Accepted

## Context

ADR 0301 has one function that decides every permission, and ADR 0304 puts it in the crate `junto-authz`, which the client can call. A client needs to know what its account may do in a channel before it offers an action: delete the message of another person, change a channel, record a call. The snapshot (ADR 0207) carries the roles of the workspace and the roles that the account holds, so a client could compute the base permissions of the account. It could not compute the permissions in a channel, because the overrides of a channel are read only by an account with "manage channel" (ADR 0208). A client that derived a permission from the roles of the workspace offered an action that a deny override refuses, and did not offer one that an allow override grants.

## Decision

Two fields of the workspace in the snapshot:

- `permissions` is the list of the workspace permissions that the account holds in the workspace: each permission for which the permission function allows a request in the workspace now.
- `channel_permissions` has one entry for each channel of the snapshot in which the account holds another list, with that list. A channel of the snapshot without an entry has the list of `permissions`.
- Each list is made by asking the permission function once for each permission of ADR 0301. No second rule exists: the server compares no role and no override for it.
- The administrator permission and the owner position pass every check (ADR 0304), so such an account has every permission in `permissions` and no entry.
- A channel that the account cannot see is not in the snapshot and has no entry.
- The server permissions of the account are in the account of the snapshot, as before.

One route for one channel:

- `GET /api/v1/channels/{channel_id}/permissions` returns the list of the caller for one channel that the caller can see. An archived channel is not in the snapshot (ADR 0410), and the people who see it read their list from this route.
- A channel that the caller cannot see, a deleted channel and a channel of a workspace that the caller is not a member of get `not_found`, as every route of a channel does.
- The route needs a session. It is in the reach of no access token (ADR 0210). It counts against the limit of reads of the account (ADR 0408).

Kept current:

- `permissions_changed` stays the one event for a change of what an account may do, and the client loads a snapshot (ADR 0208). The lists are part of the snapshot, so no further event carries them.
- A channel that appears with overrides can give an account another list than its snapshot states. Two changes do this: a fork, which copies the overrides of its source (ADR 0409), and the restore of an archived channel (ADR 0410). Both record `permissions_changed` for the accounts that see the channel, which are the accounts that receive `channel_created` or `channel_restored` for it. An account that does not see the channel receives neither.
- A channel that is created without overrides has the list of the workspace for every account, and its creation records no notice.
- The creation of a workspace records `permissions_changed` for the accounts that see the new workspace. Its creator gets the notice as a new member (ADR 0208). An account that sees the workspace without being a member, by the administrator permission, holds every permission in it, and no other event states that.

Cost:

- The snapshot reads the channels and the overrides of a workspace in two statements, as before, and computes the lists from them. It reads no further statement.
- A list has at most one name for each workspace permission. The snapshot has at most one entry for each channel that it carries, and none for a channel whose overrides do not change the list of the account.

## Consequences

- A client offers an action in a channel when the list of that channel holds its permission, and does not read roles or overrides for it. The server still decides each request.
- A client that received `channel_created` or `channel_restored` and no `permissions_changed` after it uses the list of the workspace for that channel.
- A fork of a channel with overrides, the restore of one, and the creation of a workspace cost the clients that see them one snapshot request each.
- The lists state the workspace permissions only. A rule that is no workspace permission is not in them: the deletion of a channel with permanent messages belongs to the owner (ADR 0402), and nobody edits the message of another person (ADR 0304).
- An account without the administrator permission that is no member of a workspace does not get the workspace in its snapshot, so it gets no list for it.
