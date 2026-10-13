# 0210. Access tokens: form, actions and the routes that they reach

## Status

Accepted

## Context

ADR 0203 decides that an administrator creates an access token with an expiry date and a set of actions, that each token is revoked alone, and that each request with a token is checked against the token's actions and against the current permissions of its creator. It leaves open the form of the token, what an action is, which routes a token reaches, who lists and revokes tokens, and what happens to a token when the access of its creator ends. ADR 0109 states that an access token is a credential apart from a session token. A session belongs to a device; several routes need that device: the routes of calls, of the caller's own devices and of the event stream. ADR 0301 names limits that bind an administrator and that only the server owner passes.

## Decision

Form:

- An access token is the text `jat_` followed by 32 random bytes in base64url without padding, from the random source of the operating system.
- A request carries it as `Authorization: Bearer <token>`, as a session token.
- The server stores the SHA-256 hash of the token, with a prefix that differs from the one of session tokens, and compares hashes in constant time. The creation answer carries the token once. No list, event, audit entry or log record contains it.
- A token has a name, a set of actions and an expiry. The expiry is later than the request and at most 366 days after it.

Actions and reach:

- An action is one group of routes. The actions are `read_structure`, `manage_structure`, `manage_members` and `read_audit_log`. A later record can add actions.
- One table in the server maps a method and a path to its action. A route that needs a credential and that the table does not list refuses every access token with `forbidden`, before the route runs and without reading the token. A test compares the table with the generated OpenAPI document.
- A route that needs no credential reads no `Authorization` header, of a session or of an access token: the server information, the health route, the routes of a device without a session, and the layout page (ADR 0615).
- `read_structure` reaches the reading routes of the structure: the structure document (ADR 0211), workspaces, workspace roles, channels, overrides, server roles and members.
- `manage_structure` reaches the routes that create and change workspaces, categories, channels, call settings of a channel, overrides, workspace roles and server roles, and the route that applies changes of the structure (ADR 0211).
- `manage_members` reaches the list of accounts, removal and ban, adding a member, invites, and the decision of join requests. It does not reach the assignment or the removal of a server role or of a workspace role, and not the unban: a request with a token changes no role assignment and lifts no ban (ADR 0213).
- `read_audit_log` reaches the two audit log routes.
- No token reaches the routes of sessions, of the caller's own account and devices, of access tokens, of recovery, of messages, conversations, attachments, notifications and search, of calls, of recordings and transcripts, of the snapshot and of the event stream.
- A token on a route of an action that it does not hold gets `forbidden`. An unknown, expired or revoked token on a route of the table gets `unauthenticated`.

Whose permissions:

- Creating a token needs the administrator permission and a session.
- A request with a token is decided as a request of the account that created the token, with the roles that this account has in the transaction of the request (ADR 0009). The account must be active.
- A token does not carry the owner-only permissions of the server owner. A token of the server owner acts with the administrator permission, but carries the owner rank for role-order comparisons (ADR 0309). The actions that only the owner does (ADR 0301) need a session of the owner.
- An account holds at most 100 tokens. A further creation gets `limit_reached` (ADR 0306). A token that has expired counts until the maintenance task has deleted it.

Listing and revocation:

- `GET /api/v1/access-tokens` returns the tokens that have not expired, in pages of at most 200, newest first: every token for a caller with the administrator permission, and the caller's own tokens for every other caller.
- `DELETE /api/v1/access-tokens/{id}` revokes a token. The account that created it and a caller with the administrator permission can do this. Another caller gets `not_found`, the same response as for a token that does not exist, and does not wait for the row of the token.
- A revocation deletes the row. The maintenance task deletes expired tokens.
- A ban, a removal and a recovery of an account delete every token of that account, in the transaction that ends its sessions (ADR 0109). An unban does not bring a token back.

Audit and events:

- The audit log gets `access_token_created`, with the name, the actions and the expiry, and `access_token_revoked`, with the account that created the token. Both are about the server.
- Every audit entry that a request with a token writes carries the identifier of that token in the field `access_token_id`. The transaction of such a request holds the identifier in a setting of the transaction, and the statement that writes an entry reads it, so that no route can write an entry without it.
- The events `access_token_created`, `access_token_revoked` and `access_tokens_of_creator_revoked` go to the accounts with the administrator permission. A token that reaches its expiry has no event; its expiry is in the list.

## Consequences

- A token loses access when its creator loses a permission, at the next request. A creator who lost the administrator permission keeps a token that does what the creator still may do, can list and revoke it, and cannot create another.
- A token that leaks reads no message content, no direct message, no call and no event stream, whatever its creator may read. A token with `manage_members` lets an account in, by an invite, by an approved join request or as a member of a further workspace. That account reads what every member of those workspaces reads, and nothing that a role or a lifted ban grants. A token that also holds `manage_structure` can change channel overrides and roles, and with them what every member reads (ADR 0213).
- A token cannot create, list or revoke tokens, so a leaked token cannot outlive its expiry.
- A token with `manage_structure` can give a permission to the holders of a role, as its creator can, and a token with `manage_members` can make an account a member. No token gives a role to an account. What a token gave stays after the token has ended; the audit entries name the token.
- A program cannot use a token for a function outside the four actions. ADR 0201 states that every function of the client is available to another program with an access token; this record limits the first set of actions to administration, and a later record widens it.
- A token of the server owner cannot delete a channel with permanent messages, perform a recovery or assign the owner.
- The audit log tells for each entry whether a person or a token of that person acted, and which token.
- A request with a token costs one lookup of the token and one statement that sets the setting, besides the reads of every request.
- A new action needs a migration that widens the check of the stored set, as a new permission does (ADR 0304).
- An administrator sees and revokes the tokens of other administrators and does not see the tokens themselves.

Role order and its action-specific exemptions are decided in ADR 0309.

For role-order decisions, `GET /api/v1/server-roles` accepts either `read_structure` or
`manage_members`; both still require the creator to hold `ban_members` or administrator.
This exception grants no access to the complete structure document (ADR 0309).

Workspace membership removal requires a session because it deletes the account’s workspace role assignments (ADR 0213). Role definition changes stay within manage_structure and its rank and grant checks.
