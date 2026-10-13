# 0106. Removal and bans

## Status

Accepted

## Context

A person who moderates a server needs to end another person's access to it. An account is identified only by its own identifier and its device keys, and a person can generate a new device key at any time (ADR 0103). A removed person who still holds a valid invite code could create a new account.

## Decision

- A person with the server permission `ban_members` (ADR 0301) can remove an account, ban an account and unban an account.
- A ban applies to the account. It blocks the account and its device keys: the keys cannot log in and cannot be used to join again. The invites that the account created are cancelled.
- An unban restores the same account, with its device keys, its workspace memberships and its roles. The invites that the ban cancelled stay cancelled.
- Without an unban, a banned person can come back only as a new account, through a new invite code. The audit log shows who created that code.
- Removals, bans and unbans are written to the audit log (ADR 0303). Ban and removal accept an optional reason, trimmed to at most 1000 characters without control characters; empty means no reason. An absent body and JSON null preserve the earlier no-reason request. The reason stays in the audit entry, readable only with its existing audit permission, and is not sent in events. ADR 0120 additionally gives the subject a private notice for 30 days, without the actor, containing only the separately supplied optional recipient_message and never the audit reason. Server audit readers, including tokens with read_audit_log, may read server reasons; current workspace audit readers may read workspace reasons. A returned member with that permission may therefore read the reason about the earlier removal. Ended credentials cannot retrieve audit entries; a separate device proof reads private notices under ADR 0120. Both `reason` and the separate optional `recipient_message` are retained in the audit entry. Audit readers can see whether a message was given and its text after the subject’s notice expires. Both texts are immutable with that entry and cannot be corrected in place.
- `POST /api/v1/workspaces/{workspace_id}/members/{account_id}/removal` removes just that membership. It requires administrator, protects the owner and the caller, and requires a strictly higher workspace rank under ADR 0309. The workspace audit action is `workspace_member_removed`, with the account, optional audit reason and optional recipient message. The route requires a session; tokens cannot remove workspace memberships because that removes role assignments (ADR 0213).
- Workspace removal deletes assigned workspace roles with the membership. Overrides belong to roles and stay. It ends the account's call in that workspace, abandons a pending move of that seat, and schedules media removal; permission changes recheck pending entries and contact with conversation peers. The removed account gets `workspace_member_removed`, `permissions_changed`, and `own_call_changed` if its seat ended; current workspace readers receive the member removal. Other memberships, server roles, devices and sessions stay. Client drafts are not stored by the server; a client loses access to that workspace and must stop offering its drafts there. An administrator still sees the workspace through administrator permission, irrespective of membership.

## Consequences

- A ban does not identify a person, only an account. A banned person who receives a new invite code gets a new account; the audit log shows which member created the code.
- An unban needs no invite, because it returns the account that the ban blocked. The states of an account are in ADR 0113.
- The connections of the removed or banned account receive a control event (ADR 0107).
- A removed or banned person who is in a call is removed from it through LiveKit's API (ADR 0506).
- An invite code that a banned account created stops working at the ban.

Role order and its action-specific exemptions are decided in ADR 0309.

Unban requires the actor above the account at the server level and at every workspace
level where assigned roles return; the owner always passes role order (ADR 0309). This
differs from Discord because Junto restores the account with its roles.

A workspace removal preserves authored messages, notification settings, notices, thread choices and read positions. They remain subject to current visibility and apply again after readmission; deleted role assignments are not restored. In the removal transaction, every invite created by that account loses this workspace. An invite naming no remaining workspace is revoked, with the usual revocation event and audit entry; an invite still naming other workspaces stays valid for those workspaces. A workspace has no ban, so removal must not leave the removed person a key of their own making, including for joining with a new account. An invite created by another person still permits return. Server removal is different: it marks the account removed, deletes its login credentials and leaves its invites valid; a ban is the separate rule for keeping an account out. No retained device identity participates in admission or links a new account to the removed creator in application responses. ADR 0120 retains a separate notice reader for at most 30 days.

An administrator may remove a retained workspace membership of a banned account under the same rank rule. That changes no ban or credential and the account remains banned.
