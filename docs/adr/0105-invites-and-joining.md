# 0105. Invites and joining

## Status

Accepted

## Context

A person reaches a server by its address, and the server must decide whether that person gets an account and in which workspaces. A server on a public address receives join attempts from unknown parties, so the data that a join request shows to an administrator must be limited and the number of attempts must be limited. The server may be behind another host that accepts the connections, so the address of the connection is not always the address of the client.

## Decision

Profile preset:

- The client stores a name and an avatar that the user chooses at installation.
- A new server account starts from this preset. The user can edit the name and avatar per server.

Joining:

- A person enters a server address.
- With an invite code, the account is created at once.
- Without a code, the request waits for approval by a person with the "approve join requests" permission.
- A join request carries a name only. The avatar is sent after approval.
- The waiting device asks the server for the result and proves itself with its device key.
- The limit on join attempts uses the client address that the entry host forwards. The server trusts that header only when it comes from the entry host.

Invite codes:

- Creating an invite code needs the "create invites" permission, which administrators have by default.
- Each code has an expiry date, a maximum number of uses, and the workspaces it grants.
- A code that names no workspace grants the default workspace.
- An approval of a join request names the workspaces in the same way.
- The person starts in each granted workspace as a member without roles.
- The audit log records which code created each account.

No guest accounts:

- There are no guest accounts and there is no browser client. Every participant has a normal account and uses the desktop application.

## Consequences

- A person without an account cannot join a call or read a channel. A person from outside the server installs the desktop application and joins with a code or by approval.
- An administrator who approves a request sees a name and no image.
- Only the device that made a request can collect its result, because the result is given to the holder of the device key.
- A deployment with an entry host must configure which host the server accepts the forwarded client address from. A wrong setting makes the limit on join attempts count the wrong address.
- A new member has the permissions of the "everyone" role of each granted workspace until a role is assigned (ADR 0301).
- The values of the limits on join attempts are not fixed by this record.
