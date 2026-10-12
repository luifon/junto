# 0306. Ceilings on roles, role assignments and the workspaces of an invite

## Status

Accepted

## Context

ADR 0301 and ADR 0304 set no upper bound on the number of roles of a workspace, on the number of roles that one account holds, or on the number of workspaces that one invite grants. Each of these lists is sent as a whole: in a response, in a snapshot and in an event (ADR 0208). An event has a largest size (ADR 0011), and a list without a bound can pass it. The permission function reads every role of an account for each decision.

## Decision

- A workspace has at most 250 roles, the "everyone" role included.
- A member holds at most 64 roles of one workspace, without the "everyone" role.
- An account holds at most 64 server roles.
- An invite and an approval of a join request name at most 100 workspaces. A request with more gets `invalid_request`.
- A request that would pass one of the first three ceilings gets the error `limit_reached` with the status 409, and nothing is changed.
- The server counts in the transaction of the change (ADR 0009). Two requests that each stay below a ceiling and together pass it cannot both commit: PostgreSQL ends one of them with a serialization failure, the server runs it again, and it then gets `limit_reached`.
- The ceilings are enforced by this code and not by a constraint of the database. A check constraint cannot count rows, and a trigger that counts would need a lock of its own to be correct for writers at a lower isolation level, which the server does not have (ADR 0009). The invariants that one row can state stay constraints.
- The numbers are values of the server configuration in the code. No environment variable changes them.

## Consequences

- The largest events of the server base stay far below the size that an event may have: the server roles of an account, the roles of a member, an invite with its workspaces, and the overrides that the event of a deleted channel keeps.
- A workspace that needs more than 250 roles, or a member who needs more than 64, needs a change of this record.
- A program that writes to the database directly, at another isolation level, can pass a ceiling. ADR 0009 states the same for every rule of the server.
- ADR 0309 limits the creation of server roles to 250. The number of workspaces, of channels and of invites has no ceiling. None of them is sent as one list in an event.
- A removal of an account records one event per workspace that the account was a member of, and a join one per granted workspace. Their number is bounded by the workspaces of the invite or by the memberships of the account.
