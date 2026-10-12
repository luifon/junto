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
- Removals, bans and unbans are written to the audit log (ADR 0303).

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
