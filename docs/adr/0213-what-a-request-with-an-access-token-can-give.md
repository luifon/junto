# 0213. What a request with an access token can give to an account

## Status

Accepted

## Context

ADR 0210 gave the action `manage_members` the routes that assign and take server roles and workspace roles and the route that lifts a ban. It also stated that a token that leaks gives no access to message content. The two statements did not agree. A token with the one action `manage_members` created an invite, a new account joined with it, and the token gave that account a server role of its creator. The account then read every channel with a session of its own, which outlives the token. A role is also taken to give access: a role whose channel override denies a permission keeps it from its holders (ADR 0304), and an unban gives an account back its memberships and its roles (ADR 0113). A token is held by a program, in a file or in a variable of its environment, and has no device key. A session needs the device key for each login (ADR 0108).

## Decision

No role assignment and no unban with a token:

- A request with an access token changes no role assignment and lifts no ban. These six routes are in the reach of no action and refuse every access token with `forbidden`, as every route outside the table does (ADR 0210):
  - `PUT` and `DELETE /api/v1/accounts/{account_id}/server-roles/{role_id}`;
  - `PUT` and `DELETE /api/v1/workspaces/{workspace_id}/members/{account_id}/roles/{role_id}`;
  - `DELETE /api/v1/accounts/{account_id}/ban`;
  - `POST /api/v1/workspaces/{workspace_id}/members/{account_id}/removal`.
- They need a session of an account with the permission. The rule does not depend on the role or on the account: a token gives no role at all, also not a role without permissions, because a channel override can allow a permission to any role.

What `manage_members` still reaches:

- The list of accounts, removal and ban, adding an existing account to a workspace, the invite routes, and the list and the decision of join requests.
- With them a token lets an account in: by an invite, by an approved join request, or as a member of a further workspace. That account is a member without roles. It reads what every member of those workspaces reads through the "everyone" role, and nothing that a role or a lifted ban grants.
- An account is added to a workspace only by an administrator (ADR 0304), and an invite or an approval names only workspaces that its creator may grant (ADR 0111). A token is decided as its creator (ADR 0210), so it lets an account into the workspaces that its creator could let it into.

What `manage_structure` reaches:

- The routes that change roles and channel overrides, and the route that applies changes of the structure (ADR 0211). A change of the permissions of a role, of an override or of the "everyone" role changes what the holders of that role read, and the deletion of a role removes its denies (ADR 0308). This is the purpose of the action: the setup tool writes the structure of a server with it (ADR 0204).
- Deleting a role definition, including through the structure document, is different from changing the role assignments of a named account. Definition deletion still checks the creator’s rank and the grant ceiling on the role’s deny overrides (ADRs 0308 and 0309). Adding an existing workspace member preserves its assignments; no member route may replace or delete them with a token.
- A token with `manage_structure` can therefore change what accounts read. It cannot choose one account: it names roles, and an account gets a role only through a session.

What no token reaches:

- The content itself. No token reads a message, an attachment, a notification, a search result, a recording, a transcript, a call, the snapshot or the event stream, and no token opens a direct message (ADR 0210, ADR 0302).

## Consequences

- A leaked token with `manage_members` alone gives a new account the access of a member without roles in the workspaces that the creator of the token may grant. It does not give the administrator permission, a role, or the access of a banned account.
- A leaked token with `manage_structure` can open a private channel to the holders of a role, or to every member, by a change of an override or of a role. The audit log names the token in each of these entries (ADR 0210). An operator who gives this action to a program gives it the structure of the server.
- A program that assigns roles to accounts cannot use a token. It needs a person with a session, or a later record that adds an action for it with its own rule.
- The setup tool is not affected: the structure file names no account and no member (ADR 0211).
- A test gives a token every action and requests each of the six routes; each answers `forbidden`, and the same request with the session of the creator is allowed.
