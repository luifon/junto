# 0113. Account states for removal and ban

## Status

Accepted

## Context

ADR 0106 states that an administrator can remove a member and can ban and unban an account, that the keys of a banned account cannot log in, and that a returning person gets a new account. Audit entries and later messages refer to accounts, so an account row cannot disappear. The record does not state what is stored for a removed or a banned account.

## Decision

- An account has one of the states active, banned, removed, and the states inactive and deleted of ADR 0121, which the person reaches by leaving the last workspace or deleting the account. The account row is never deleted.
- Removal, ban and unban need the server permission `ban_members` (ADR 0304).
- Each of the three is one serializable transaction (ADR 0009). A request of the affected account that overlaps it either finishes before it and is covered by it, or sees its result.

Ban:

- A ban sets the state to banned, deletes every session of the account and cancels every invite that the account created.
- The devices, the memberships and the roles of a banned account stay stored.
- The key of a banned account gets no `login` challenge, and it cannot join or ask to join, because it is still registered.
- An unban sets the state to active. It restores the same account: the account logs in again with the same keys and has the same memberships and roles. The cancelled invites stay cancelled.
- Without an unban, the banned person can come back only as a new account, through a new invite code and with a new device key (ADR 0106).

Removal:

- A removal sets the state to removed and deletes the devices, the sessions, the workspace memberships and the role assignments of the account.
- A removed account cannot be banned, unbanned or recovered, and cannot become the server owner (ADR 0114).
- The invites that a removed account created stay valid.

Limits:

- The server owner cannot be removed or banned.
- An account cannot remove or ban itself.

## Consequences

- A removed person can join again with a valid invite code and the same device key, and gets a new account.
- A banned person needs a new device key and a new invite code to return, and gets a new account; the audit log shows who created that code.
- An unban restores access without a new invite. This is the one way back into the banned account; ADR 0106 states the same rule.
- The list of accounts shows banned, removed, inactive and deleted accounts with their state.
- An inactive account can be removed and not banned; a deleted account takes no removal, ban or unban (ADR 0121).
- The name of a removed account stays stored for the audit log and for later content that refers to it.
