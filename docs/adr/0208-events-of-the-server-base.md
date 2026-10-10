# 0208. The events of the server base and who receives them

## Status

Accepted

## Context

The server base changes accounts, devices, invites, roles, workspaces and channels (ADR 0903). Each change needs an event for the people who may see it, and nobody else may learn of it (ADR 0303, ADR 0304). ADR 0107 names control events that also reach background connections: permission changes, removals, bans, deletions and the server's version. A change of a role or of a channel override can change which channels a member sees, and the event about it must not name a channel that the member cannot see.

## Decision

Audiences:

| Events | Recipients |
|---|---|
| `device_added`, `device_renamed`, `device_removed`, `device_logged_in`, `device_logged_out`, `account_recovered` | The connections of the account itself |
| `permissions_changed` | The accounts whose permissions the change can have altered |
| `account_added`, `account_status_changed`, `account_server_roles_changed`, `owner_changed` | Accounts with `ban_members` |
| `server_role_created`, `server_role_updated` | Accounts with `administrator` |
| `invite_created`, `invite_used`, `invite_revoked`, `invites_of_creator_revoked` | Accounts with `create_invites` |
| `join_request_created`, `join_request_decided` | Accounts with `approve_join_requests` |
| `workspace_created`, `workspace_updated`, `workspace_member_added`, `workspace_member_removed`, `workspace_member_roles_changed`, `workspace_role_created`, `workspace_role_updated`, `category_created`, `category_updated` | Accounts that may see the workspace |
| `channel_created`, `channel_updated`, `channel_deleted` | Accounts with `view_channel` in the channel |
| `channel_override_set` | Accounts with `manage_channel` in the channel |

- Each audience is the permission that the REST route for the same data needs (ADR 0304).
- The decision is made when the event is delivered (ADR 0011).
- An event carries the whole changed object, so a client replaces what it has.
- A change that creates several objects records an event for each. The creation of a workspace records `workspace_created` and `workspace_role_created` for its "everyone" role.
- A ban cancels every invite of the account. It records one `invites_of_creator_revoked` with the account, not one event per invite (ADR 0011).

Permission changes:

- A change that can alter what an account may see or do records the notice `permissions_changed`. The notice names the workspace, or no workspace for the permissions of the server. It names no role and no channel.
- The client that receives the notice loads a snapshot (ADR 0207).
- The notice goes to the account when its membership, its roles or the owner position change. It goes to the holders of a role when the permissions of the role or a channel override of the role change; for the "everyone" role these are all members of the workspace.
- A change of the name of a role records no notice.

Classes:

- Control events reach every level (ADR 0107): `permissions_changed`, the events of the account's own devices and sessions, `account_recovered`, `workspace_member_removed` and `channel_deleted`.
- All other events of this record are general and reach the full level only.
- The classes for direct messages, mentions and call invites reach the background level. The features that add those add their events.
- The version of the server is in `hello` (ADR 0206). It changes only with a restart, which closes every connection.

Removal and ban:

- The account that is removed or banned gets the message `ended` with the reason, and its connections close (ADR 0011). The accounts with `ban_members` get `account_status_changed`.

Recovery:

- A recovery ends every session of the account (ADR 0112). The connected devices of the account get `ended` with the reason `recovered` and the recovery, and their connections close.
- The notice of the device (ADR 0112) is not used for this, so a login and an acknowledgement of the notice (ADR 0118) do not change the reason.

Reason of an ended session:

- A change that ends a session before its expiry stores a row for the session in the same transaction: the identifier of the session, the reason (logged out, device removed, banned, removed, recovered) and, for a recovery, the recovery.
- A connection whose session no longer counts reads the row of its session and sends that reason. The server does not derive the reason from the order of identifiers or of times.
- A session without a row has expired. The connection then sends `session_ended`, or the reason that the state of the account and of the device gives.
- The maintenance task (ADR 0009) removes a row when the session would have expired, 30 days after it was ended at the latest (ADR 0109).
- The event `account_recovered` and the event `device_added` of the new device stay in the log. A device that logs in again and subscribes with its cursor receives them.
- The notice in the response to the login (ADR 0112) stays until the device acknowledges it (ADR 0118). The three carry the same recovery identifier, and a client shows a recovery once.

No events:

- Audit entries are not sent. They are read from the audit log (ADR 0305).
- The expiry of an invite, a join request, a session or a device link records no event. The client knows the time from the object.
- A challenge, a request for a device link and an issued owner code record no event.
- The first start of a server records no event.

## Consequences

- A member who cannot see a channel receives no event that names it: not its creation, its changes, its overrides or its deletion.
- The deletion of a private channel is told to the accounts that could see it, although the deletion removes its overrides (ADR 0407), because the event keeps them (ADR 0011).
- A channel is created without overrides and is visible to every member until an override hides it. Members who are connected in between receive its creation.
- A member receives `permissions_changed` when an override of one of its roles changes in a channel that the member cannot see. The member learns that a permission of the workspace changed, and not where.
- A permission change costs the affected clients one snapshot request.
- A background connection learns of the devices and recoveries of its account, which ADR 0107 does not list. They are few and concern the security of the account.
- An event type and its audience are fixed in one place of the code, so a new event cannot be recorded without a decision about its recipients.
