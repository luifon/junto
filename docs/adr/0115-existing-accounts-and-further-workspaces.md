# 0115. Adding an existing account to further workspaces

## Status

Accepted

## Context

An account belongs to the server and membership is per workspace (ADR 0102). An invite or an approval creates an account in the workspaces it grants (ADR 0105). The records gave an account that already exists no way into another workspace of the same server.

## Decision

Redeeming an invite:

- A logged-in account sends an invite code to `POST /api/v1/invites/redemption`.
- The account becomes a member of each workspace of the invite that it is not yet a member of, without roles.
- The invite counts one use with the same conditional update as a join, so the expiry, the cancellation and the maximum number of uses apply in the same way (ADR 0111).
- A redemption that adds no workspace gets `conflict` and counts no use.
- Attempts are limited per client address and per account, 10 per 10 minutes each (ADR 0111). The limits are checked before the server reads or locks the invite, so repeated attempts with wrong codes or without effect cannot hold up joins.
- The audit log gets the entry `invite_redeemed` about the server, with the account, the invite, the creator of the invite and the added workspaces.

Adding directly:

- A person with the administrator permission adds an active account to a workspace with `PUT /api/v1/workspaces/{workspace_id}/members/{account_id}`.
- The route is idempotent. The audit log of the workspace gets the entry `workspace_member_added` when the membership is new.
- A banned or removed account is refused with `conflict`.

## Consequences

- One invite serves new people and existing accounts.
- A person with "create invites" can let an existing account into the workspaces that the person is a member of, and no others (ADR 0304).
- A direct addition needs no invite and no action by the added account.
- Removing an account from one workspace only is not part of this record.
