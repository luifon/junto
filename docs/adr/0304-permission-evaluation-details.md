# 0304. Details of the permission function: view permission, grant rule and stored sets

## Status

Accepted

## Context

ADR 0301 fixes the two levels of roles, the evaluation order, the administrator permission and its limits. An implementation needs rules that it leaves open: which permission makes a channel visible, what exactly "nobody grants a permission they do not hold" covers, which permission the actions without a named permission need, which workspaces an invite can grant, and how a set of permissions is stored.

## Decision

One function:

- The crate `junto-authz` has one function that takes the caller, a request and the roles and overrides that the request needs, and returns allow or a reason for the denial. Every route calls it. A route contains no comparison of roles or permissions.
- The function runs in three steps: the limits, the administrator permission, the roles. A request that a limit decides never reaches the second step.
- The server owner passes every check that the administrator permission passes.

View permission:

- The workspace permission "view channel" is added to the list of ADR 0301. A private channel is a channel whose override of the "everyone" role denies it.
- Every permission in a channel needs "view channel" in that channel as well.
- The "everyone" role of a new workspace has "view channel", "lock room" and "run meeting mode".

Membership:

- A request inside a workspace is denied to an account that is not a member of it, unless the account has the administrator permission.

Grant rule:

- A change is checked against the permissions that it can add to any account:
  - creating a role: the permissions of the role;
  - editing a role: the permissions that the edit adds;
  - assigning a role: the permissions of the role, and for each channel the permissions that the channel overrides of the role allow there;
  - removing a role from a member, and deleting a role (ADR 0308): for each channel the permissions that the channel overrides of the role deny there, because the member gets them back;
  - changing a channel override: the permissions added to the allow set and the permissions removed from the deny set.
- The caller must hold each of those permissions: in the workspace for the permissions of a role, in the channel for the permissions of an override. A caller who cannot see a channel holds nothing in it.
- Removing a permission from a role and adding a deny need only the managing permission. So does removing a role whose overrides deny nothing.
- The rule for the removal of a role is the same for every target: the caller's own account and another member.

Managing permissions:

- Workspace roles need "manage roles". Channels, categories and channel overrides need "manage channel".
- Server roles, renaming a workspace and adding an existing account to a workspace (ADR 0115) need the administrator permission, because ADR 0301 names no other permission for them.
- Removal, ban and unban need "ban members". The server owner and the caller's own account cannot be the target (ADR 0113).
- Creating an invite or approving a join request needs its server permission, and the caller must be a member of every workspace that it grants.
- The account that creates a workspace becomes its first member and gets no role in it.

Message edits:

- Editing a message is denied when the message is permanent, when the caller is not its author, and in a direct message when the caller is not a participant. These are limits. No permission lets a person edit the message of another person.

Stored sets:

- A set of permissions is stored as one integer in which each permission has a fixed bit. A test fixes the bit and the name of every permission.
- The database refuses an integer with a bit above the highest known one. A new permission needs a migration that widens that check.
- Overrides exist for channels only. A category has no overrides.

## Consequences

- One test suite of table tests and property tests covers the rule for every route.
- A person with "manage roles" cannot use a role to enter a channel: assigning a role whose override allows "view channel" in a channel that the person cannot see is denied, and so is removing a role whose override denies "view channel" there, from the own account or from another member.
- The roles and overrides that a decision uses are read in the transaction of the change (ADR 0009), so a permission that was revoked before the change commits is not used.
- A person with "manage channel" who denies a permission to a role that the person holds loses that permission in the channel and cannot remove the deny again. An administrator can.
- Role changes and server moderation also obey the order of ADR 0309. A person with "manage roles" changes only roles below their highest role, with the permission-grant ceiling above.
- A person with "create workspace" and no administrator permission gets an empty workspace that an administrator has to give roles to.
- A category does not hide its name from members who see none of its channels.
- The number of a permission bit cannot be reused for another permission.
