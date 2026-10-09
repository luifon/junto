# 0106. Removal and bans

## Status

Accepted

## Context

An administrator needs to end a person's access to a server. An account is identified only by its own identifier and its device keys, and a person can generate a new device key at any time (ADR 0103). A removed person who still holds a valid invite code could create a new account.

## Decision

- An administrator can remove a member.
- An administrator can ban an account. The device keys of a banned account cannot log in, and the invites that the account created are cancelled.
- An administrator can unban an account.
- A ban applies to the account.
- A banned person can return only with a new invite code, and the audit log shows who created that code.
- Banning needs the "ban members" server permission (ADR 0301).
- Removals and bans are written to the audit log (ADR 0303).

## Consequences

- A ban does not identify a person, only an account. A banned person who receives a new invite code gets a new account; the audit log shows which member created the code.
- The connections of the removed or banned account receive a control event (ADR 0107).
- A removed or banned person who is in a call is removed from it through LiveKit's API (ADR 0506).
- An invite code that a banned account created stops working at the ban.
