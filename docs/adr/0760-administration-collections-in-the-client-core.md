# 0760. Administration collections in the client core

## Status

Accepted

## Context

The snapshot carries the structure of the workspaces and not the collections that a person with the permissions manages: accounts, invites, join requests, access tokens, exports, server roles, the overrides of a channel, the archived channels and the settings of a workspace (ADR 0207). The server tells the changes of most of them as events that carry the changed object. `retention_changed` names a workspace and no value. ADR 0719 reads the members of a workspace on demand and applies the events of the time of the read after the answer. Server units added the events `access_token_created`, `access_token_revoked`, `access_tokens_of_creator_revoked`, `channel_archived`, `channel_restored`, `channel_deletion_due`, `category_deleted`, `workspace_role_deleted`, `server_role_deleted`, `workspace_deleted`, `export_updated` and `export_removed`, the routes of ADR 0210, 0308, 0410 and 0411, and the field `access_token_id` of an audit entry. The grant rule of ADR 0304 refuses a change of an override that adds a permission that the caller does not hold in the channel.

## Decision

Routes:

- The API object of a server has one function for each route of ADR 0308 (deletions), ADR 0410 (archive, restore, archived list, archive period), ADR 0409 (file, recording and direct-message retention), ADR 0411 (exports and the file of an export from a byte) and ADR 0210 (access tokens).

Events of the structure:

- `workspace_deleted` removes the workspace from the state with everything that the state holds of its channels: timelines, summaries, pin lists, waiting messages, and the invitations, requests and moves of their calls. The state is not reloaded.
- `category_deleted` removes the category and clears it on its channels. `workspace_role_deleted` removes the role from the workspace, from the roles of the account, from the loaded members and from the loaded overrides. `server_role_deleted` removes the role from the account, from the loaded server roles and from the loaded accounts.
- `channel_archived` takes the channel out of the channel list and ends what waits for its call. `channel_restored` puts the channel back as `channel_created` does.

Collections:

- The state of a server holds the administration collections in one shared part, so a copy of the state does not copy them. A collection is not loaded, loading, loaded, or failed with a reason: not allowed, not found, unavailable, or no answer.
- The collections are: accounts, invites, the join requests that wait, access tokens, the exports of the account, server roles, the retention of direct messages, and per channel the overrides, per workspace the archived channels and the settings (archive period, file retention, recording retention).
- A reader opens a collection and releases it; the client counts the readers. The first reader starts the read from the route. The last release drops the collection and ends a read that is on its way.
- A collection is a part of the state in the sense of ADR 0719: the events that arrive while it is read are kept and applied after the answer, in their order. They are applied to the collections only: the rest of the state followed each event when it arrived, and a kept event that is applied to the whole state a second time would undo a later event of the same object. An event changes a collection only while it is loaded.
- The members of a workspace (ADR 0719) keep `workspace_role_deleted` during their read too, so a role that is deleted while the members are read is not in the answer afterwards.
- A collection whose workspace or channel leaves the state while its read is on its way is closed when the answer arrives, and a reader that waits is told so.
- After a reload of the state every open collection is read again. A collection whose workspace or channel the new state does not hold is closed, and a reader that waits for it is told so. A change of the session sends an interrupted read again.
- A failed read stays in the state with its reason. A reader asks for the read again; the client does not repeat it by itself.
- The settings of a workspace and the retention of direct messages are read again after `retention_changed`, because the event carries no value. The earlier values stay in the state while the read runs. When the event arrives while a read is on its way, that read is not ended and one more read follows its answer, so a row of such events cannot keep the collection unread. The client remembers that the new read of a loaded collection is owed, so a change of the session sends it again although the state shows the earlier values.
- `workspace_deleted` also removes the notification settings of the snapshot that name the workspace or one of its channels that the state holds. A setting of an archived channel that the state does not know stays until the next snapshot: the state cannot tell which workspace that channel belonged to. The call of the account follows `own_call_changed` of the server, as after `channel_deleted`.
- The access tokens are read in pages of 200 until the server names no further page, a page adds no token, or 2,000 tokens are held. The state says when the server holds more.
- A use count of an invite never goes down: an event that is applied a second time after an answer does not lower it.
- The audit log is not a collection. A reader asks for a page of it, as for a search (ADR 0722).

Permissions in a channel:

- The client core depends on `junto-authz`, the crate with the permission function of the server. With the roles of the account and the loaded overrides of a channel it computes the permissions that the account holds in that channel, by the same function that the server uses for the grant rule. An administrator holds every permission. Without the loaded overrides the answer is unknown.

Command-line tool:

- `junto-cli` has commands for the routes of the administration: workspaces, categories, channels with archive and call settings, overrides, members, accounts with removal and bans, invites, the redemption of an invite, the denial of a join request, access tokens, exports with the download, the audit log, the archive period and the retention. `watch` prints the new events.

## Consequences

- A page of the administration shows a change that another person made without a request of its own.
- A collection that nobody has open costs no request and no memory.
- A reader that forgets a release keeps a collection in the state and its events applied.
- The permissions of the account in a channel are known only to a person who may read the overrides, which needs "manage channel". For every other person the client shows what the roles of the workspace allow, and the server decides.
- The token list of a server with more than 2,000 tokens is not complete on screen.
- The client applies the permission function of the server to predict a refusal. The server still decides every request (ADR 0304).
