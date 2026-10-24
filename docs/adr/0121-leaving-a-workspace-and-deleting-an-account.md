# 0121. Leaving a workspace and deleting one's own account

## Status

Accepted. Adds the account states `inactive` and `deleted` to ADR 0113, the profile of a departed account to ADR 0119, one event to ADR 0208, two open sets of values to ADR 0209, and the read-only direct conversation to ADR 0307 and ADR 0408.

## Context

Until this record only another person could end an account's access: an administrator removes a workspace membership (ADR 0106), and a person with `ban_members` removes or bans an account (ADR 0113). A person had no way to leave a workspace, to leave a server, or to end their own account.

Messages, files, reactions, threads, direct conversations, recordings, transcripts and audit entries refer to accounts by identifier. Other people read that content after its author has gone, and audit readers need to know who did what. So an account row is never deleted (ADR 0113), and content and audit entries must stay when the person leaves or deletes the account.

The server owner holds every permission and is the only position that can recover an account (ADR 0104). The position changes only through the host command `assign-owner` (ADR 0114).

## Decision

### Terms

- **Leave a workspace**: the account ends its own membership of one workspace.
- **Inactive**: the state of an account that left the last workspace of which it was a member. The person can return to the same account.
- **Deleted**: the state of an account that its person deleted. It is final.
- **Departed account**: an inactive or a deleted account.
- **Return**: an inactive account becomes active again.

### Account states

ADR 0113 gives an account the states active, banned and removed. This record adds two:

| State | Login | Devices | Memberships, roles, server roles | Name | Avatar |
|---|---|---|---|---|---|
| inactive | refused with `account_inactive` | kept | none | kept | kept, shown to nobody else |
| deleted | refused as an unknown device | deleted | none | kept | deleted |

The account row and the name stay in both states, for the audit log and for the content that names the account.

### Leaving a workspace

`POST /api/v1/workspaces/{workspace_id}/departure`, with no body, answers 204.

- It needs a session. An access token is refused (ADR 0210, ADR 0213): leaving deletes role assignments.
- The caller must be a member of the workspace; otherwise `not_found`.
- It is one serializable transaction (ADR 0009) and does what the removal of a membership does (ADR 0106), through the same function:
  - the membership and its workspace role assignments are deleted; the overrides of the roles stay;
  - every invite that the account created and that is still valid (not revoked, not expired, not used up) loses this workspace; an invite that names no other workspace is revoked, with its event and audit entry (ADR 0106);
  - a seat of the account in a call of a channel of the workspace ends, with a pending move of that seat;
  - `permissions_changed` for the account, and for its direct conversation peers (ADR 0208);
  - `workspace_member_removed` for the accounts that see the workspace and for the account.
- No moderation notice is recorded (ADR 0120): nobody moderated the account.
- The server stores that the account left this workspace by itself (`account_departures`). The members of the workspaces that an account left by itself are the members who see its departure (below).
- Audit entry `workspace_member_left` about the workspace, with the account. The acting account is the account itself.
- Every other membership, the server roles, the devices and the sessions stay, unless this was the last membership (below).
- The default workspace can be left as any other.

### Leaving the last workspace: the account becomes inactive

When the membership was the last of the account, the same transaction also:

- sets the state to `inactive` and stores the time of the departure (`accounts.departed_at`);
- ends every session of the account, with the stored reason `inactive` (ADR 0208), and with them every seat in a call, a direct call included (ADR 0510);
- deletes every access token of the account (ADR 0210);
- deletes every server role assignment of the account;
- revokes every invite of the account that is still valid (not revoked, not expired, not used up), with an `invite_revoked` event and an `invite_revoked` audit entry for each, whose acting account is the account. Reason: an invite admits people in the name of its creator, and the creator can no longer revoke it or answer for it;
- ends the exports of the account (ADR 0411);
- records `account_status_changed`, `account_server_roles_changed` when the account held a server role, and `account_departure_changed`;
- records the audit entry `account_left` about the server.

The devices and their keys stay registered, so that the same keys can return to the account. Recovery notices and moderation notices keep their own rules (ADR 0118, ADR 0120).

An account that is a member of no workspace, for example after administrators removed every membership, stays active; it has no workspace to leave and can delete itself.

### Deleting one's own account

`DELETE /api/v1/account`, with no body, answers 204.

- It needs a session. An access token is refused.
- The account must be active. A request of a banned, removed, inactive or deleted account has no session (ADR 0113).
- It is one serializable transaction. It ends each membership as leaving a workspace does, with a `workspace_member_left` audit entry for each workspace, so that a reader of a workspace log sees the member leave. Then it does everything that leaving the last workspace does, with the stored session reason `deleted`, and also:
  - deletes every device of the account; the keys are free, and a key that joins again later makes a new account (ADR 0113);
  - deletes the avatar;
  - sets the state to `deleted`;
  - records the audit entry `account_deleted` about the server, with the workspaces that the account left.
- The name, the account row, the notification settings, the read positions and the thread choices stay. Nothing reads them while the account is deleted.

### The owner and the last administrator

- The server owner cannot leave the last workspace of which it is a member and cannot delete its account: both get `owner_protected`. Another account must first become the owner with `assign-owner` (ADR 0114). The owner can leave a workspace while it stays a member of another.
- An inactive or a deleted account cannot become the owner: `assign-owner` already refuses an account that is not active.
- There is no rule for the last administrator. The owner holds every permission and stays active, so a server always has an account that can administer it. An administrator, also the only holder of the administrator permission, can leave its last workspace or delete its account, and its server roles end.

### Returning to an inactive account

- A device whose key is registered to an inactive account joins with an invite code (`POST /api/v1/join`) or asks to join (`POST /api/v1/join-requests`), as a new person does.
- Join with an invite: in one transaction the invite counts one use (ADR 0111), the account becomes active, it becomes a member of the workspaces of the invite without roles, and a session starts for the device of the key. The stored name stays: the name of the request is not used. Audit entry `account_returned` with the account, the invite and the creator of the invite, in place of `account_joined`.
- Join request: the request carries `returning_account_id`, so that the people who approve requests see that the key belongs to an inactive account, whose name the profile route gives (below). An approval makes that account active in the approved workspaces and records `join_request_approved` with that account and `account_returned` with the request. The device then logs in with its key, as after any approval.
- At the approval the server looks up the key again: an unregistered key creates an account as before; a key of an account that is not inactive gets `key_already_registered`.
- Events of a return: `account_status_changed`, `account_departure_changed` with no departure, and the events of new memberships (ADR 0208).
- The server roles, the roles in workspaces and the invites that ended stay ended.

### Moderation of departed accounts

- An inactive account can be removed (ADR 0113): its devices are deleted, the keys are free, and it can no longer return. It cannot be banned: `conflict`. A person with `ban_members` who wants to keep the person out removes the account; a return then needs an invite and gives a new account.
- A deleted account can be neither removed nor banned nor unbanned: `conflict`.
- The list of accounts (ADR 0113) shows both states.

### What other members see

- The profile of a departed account (`GET /api/v1/accounts/{account_id}/profile`) has the field `departure`, with the value `inactive` or `deleted`, and no avatar. It is shown to the other members, as the operator's rule names them: the current members of the workspaces that the account left by itself, the accounts with a direct conversation with it, and the accounts with `ban_members` (ADR 0119). Another account gets `not_found`, as for an unknown account (ADR 0205). Reason: these accounts read content that the account wrote; without the profile they could not tell a departed author from an unknown one.
- The avatar route answers `not_found` for a departed account, to every caller other than the account itself.
- The profile of an active account has no `departure`. A ban and a removal are not stated in a profile (ADR 0113).
- Earlier `account_profile_updated` events of the account carry its avatar; they keep the audience of ADR 0119, decided at delivery, and so reach no account without a common workspace after the departure.
- The window shows the stored name with a mark wherever it names a departed person: "Ana (inactive)", "Ana (deleted account)". It shows the generated picture of the account, never the stored avatar, and the readable alternative of the picture states that the account left the server or was deleted.
- Member lists no longer contain the account. Mentions, the contact list and call invitations offer only active accounts (ADR 0307).
- A direct conversation with a departed account stays readable for the other participant; see the next section.

### Direct conversations with a departed account

ADR 0408 hides the whole history of a direct conversation while the contact rule of ADR 0307 fails. For a conversation whose other participant is inactive or deleted, the remaining participant keeps read-only access instead:

- Readable: the list of conversations, the history, a single message, the files of its messages, its search results, the read position of the remaining participant, and an export.
- Refused with `not_found`, as for a conversation that does not exist: a new message, an edit, a deletion, a reaction, a pin, an upload, and opening the conversation again. A direct conversation has no threads (ADR 0413).
- Each server route that opens a direct conversation states whether it reads or changes it (`messages::Purpose`), so that a change never passes through a read-only conversation. The rule of search, notices and the snapshot (`access::visibility`) lists such a conversation as readable.
- The departed account itself has no session and reads nothing.
- A removal by a moderator after the departure (ADR 0113) keeps the conversation read only: the time of the departure stays stored with the removed account.
- After a return the contact rule decides again (ADR 0307).
- The window marks the other participant in the list of conversations, shows the messages as offering no action, and states in place of the composer that the conversation stays readable and that nobody can write in it.
- An event about a message of such a conversation, for example a removal by retention (ADR 0406), still needs the contact rule at delivery (ADR 0208); the remaining participant sees that change at the next read of the history.

### Event

`account_departure_changed` carries the account and its departure: `inactive`, `deleted`, or none after a return.

- Audience: the members of the workspaces that the account left by itself and its direct-conversation peers, listed in the transaction of the change and delivered from that stored list (as ADR 0308 does for former members), and the accounts with `ban_members`, decided at delivery (ADR 0208). Two events with the same content are recorded; a client that receives both applies the same change twice. A client updates the name and the picture that it holds for the account.
- Class: general.

The account itself gets `ended` with the new reason `inactive` or `deleted` (ADR 0206, ADR 0208), and its connections close.

### Audit entries

| Action | Level | Fields |
|---|---|---|
| `workspace_member_left` | workspace | account |
| `account_left` | server | account |
| `account_deleted` | server | account, workspaces that it left |
| `account_returned` | server | account, invite and its creator, or join request |

The acting account of each is the account itself, except for `account_returned` after a join request, whose acting account is the approver. Every earlier audit entry stays: the audit log refuses changes (ADR 0305), and the account rows that it names are never deleted.

### Open sets of values

- `AccountStatus` gains `inactive` and `deleted`, and the reason of `ended` gains `inactive` and `deleted`. No released client reads API version 1 yet, so both sets change within version 1.
- From this record on, a reader reads a status and a reason that it does not know as `unknown`, as it reads error codes and audit actions (ADR 0209). A later state or reason can then be added within the version. A client treats an unknown reason of `ended` as an ended session, and an unknown status as an account that it cannot act on.
- The new error code `account_inactive` (403) is an addition that ADR 0209 allows.

### The client on the person's own machine

The client keeps no copy of messages or files on disk. Its local data of a server is the entry of the server book, the device key of the server, and settings such as the volumes of people.

- Leaving a workspace that is not the last: the client drops the workspace from what it holds, as for a removal.
- Leaving the last workspace: the book keeps the entry and the key, with the membership `inactive`. The window shows that the person left the server, a field for an invite code to return to the same account, and the control that removes the server from the list. Removing it deletes the key; a later join then makes a new account, and the window says so before it removes the entry.
- Deleting the account: the client removes the entry and the key of the server from the book, because the key no longer belongs to an account. Removal waits for a running access action or session renewal and holds the same lock until the removal finishes. It belongs to the entry captured before the deletion request, so a later replacement entry and its key stay. A successful deletion completes this local removal even when the person has closed the account page while its answer was on the way.
- Another device of the same person: the session ends with the reason `inactive` or `deleted`. For `inactive` the book entry gets the membership `inactive`, as above. For `deleted` it gets the membership `deleted`, and the window states that the account was deleted and offers to remove the server from the list. A later failed login with an unknown key does not change `deleted` into the state of a removed account.
- A request to return keeps the membership `inactive` and its account identifier in the book. The pending request is shown in memory; a restart checks its status with the retained device key. A denial or expiry restores the inactive screen without clearing the membership or the key. A check first probes the account, so a return approved on another device is recognized even when an older request was denied or expired.
- A login answer `account_inactive` gives the membership `inactive`.
- The server book has format version 5 for the two new memberships. An older client refuses the file instead of dropping the entries (ADR 0710).

### Window controls and confirmation

- The account layer (ADR 0781) has the page "Leave or delete". It lists the workspaces of which the account is a member, each with "Leave", and offers "Delete account".
- The server menu offers "Leave workspace" for the shown workspace. It opens that page with the question for that workspace.
- For the owner, "Leave" of the last workspace and "Delete account" are disabled, with the sentence that another account must first become the owner through the host command.
- Each control opens a question in its row that states what stays and what ends before the person confirms:
  - Stays: the messages, files, reactions, threads, direct conversations, recordings and transcripts in which the person took part, under the person's name with the mark, and every audit entry.
  - Ends when leaving a workspace: the membership and its roles; invites created by the person for that workspace.
  - Ends when leaving the last workspace: also the sessions of every device on the server, the server roles and the remaining invites; the account becomes inactive and can return with an invite code.
  - Ends when deleting: also the devices, the picture and the account itself; it cannot be undone; the server is removed from the list on this device.

### Setup tool, access tokens and API documents

- The structure file contains no members (ADR 0212), so the setup tool does not change. A token of a departed account is deleted with the account's other credentials, and the setup tool then gets `unauthenticated`.
- The two new routes are in the reach of no token action (ADR 0210).
- `docs/api/openapi.json` and `docs/api/events.json` are regenerated with the routes, the fields `departure` and `returning_account_id`, the event, the statuses, the reasons, the error code and the audit actions.

## Consequences

- A person can leave and come back without help from an administrator, other than an invite or an approval.
- Content stays readable and attributed after its author left or deleted the account; readers see that the author is no longer there.
- The members of the workspaces that an account left, its conversation peers and the moderators learn of its departure. Other accounts learn nothing new; nobody learns of bans and removals through it.
- An inactive account keeps its public keys on the server. They grant nothing while the account is inactive, other than a return with an invite or an approval.
- A deleted account cannot be restored. Its person can join again only as a new account, through an invite or an approval.
- An owner who wants to leave needs host access, through `assign-owner`.
- A departed account's audit history, name and content stay in backups as before.

Ordinary removal from the server book also waits for a running access action or session renewal. Selection belongs to an entry generation, so cleanup of a removed entry cannot clear selection of a replacement with the same server identifier. Account deletion keeps its original entry and account through local cleanup; a late answer changes the account page and removal result only for that entry and request. A replacement page and its pending operation are independent of the old deletion.

The cancellation of a waiting request also belongs to the entry generation that it was started for. Its completion ends that cancellation before it checks whether it still owns the removal: when the same server was added again and a removal of the new entry replaced the removal of the cancelled entry, the completion leaves the new removal alone, and the server identifier is not left marked as cancelling. A later entry of the server can then give up its own request, and its access controls are not shown as busy because of an older entry.
