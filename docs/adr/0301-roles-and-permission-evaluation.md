# 0301. Two levels of roles and one evaluation rule

## Status

Accepted

## Context

A server hosts several workspaces (ADR 0102), and each workspace needs its own roles. An invite, an approval of a join request, a ban and the server audit log are not inside any one workspace, so a workspace role cannot hold the permissions for them. Permissions are checked in several places: the API, the event stream, downloads, exports, search and call admission. Two implementations of the check can give two different answers for the same person and channel. A permission that passes every check must still not open direct messages or change a permanent message.

## Decision

Two levels of roles:

- Server roles hold the permissions that act on the whole server.
- Workspace roles hold all other permissions. They are created per workspace as needed.
- Server permissions: administrator, create workspace, create invites, approve join requests, ban members, view the server audit log.
- Workspace permissions named so far: manage roles, manage channel, manage messages, mention everyone in a channel, manage glossary, record calls, manage recordings, export channel, move members, mute members, deafen members, disconnect members, lock room, run meeting mode, view the workspace's audit entries.
- Defaults: administrators have "create invites" and "export channel"; all members have "lock room" and "run meeting mode".

Channel overrides:

- A workspace role can have allow overrides and deny overrides per channel.
- Overrides exist per channel only. A category has no overrides, and a channel takes no override from its category.
- There are no overrides for a single member in the first version.

Evaluation rule:

- A member's base permissions in a workspace are those of the workspace's "everyone" role plus those of all of the member's roles in that workspace.
- In a channel, the override of the "everyone" role applies first, then the denies of the member's roles, then the allows of the member's roles. An allow from one role wins over a deny from another role.
- One function applies this rule for the API, events, downloads, exports, search and call admission.

Administrator permission and its limits:

- The administrator permission passes the checks above and sees every channel, including private channels.
- These limits are checked before the administrator permission and apply to administrators too:
  - Direct messages are closed to everyone except their two participants (ADR 0302).
  - Nobody edits a permanent message (ADR 0402).
  - Recovery (ADR 0104), assigning the server owner, and deleting a channel that contains permanent messages (ADR 0402) belong to the server owner only.

Invitations:

- An invitation never passes a permission. An invitation into a voice channel works only for a person who already has access to that channel. It passes the lock and the user limit, and nothing else (ADR 0504).

## Consequences

- A person, an event, a download, an export, a search result and a call admission are all decided by the same function, so one test suite covers the rule for all of them.
- A member with two roles, one that denies and one that allows a permission in a channel, has the permission.
- A channel cannot be hidden from an administrator.
- An exception for one member in one channel requires a role that only that member has.
- The server owner is a single position above the administrator permission. It is assigned by the first start of the server and by the server command line (ADR 0201).
- The list of workspace permissions is not closed; later records can add to it.

Role order and its action-specific exemptions are decided in ADR 0309.
