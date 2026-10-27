# 0306. Ceilings on roles, role assignments and the workspaces of an invite

## Status

Accepted

## Context

ADR 0301 and ADR 0304 set no upper bound on the number of roles of a workspace, on the number of roles that one account holds, or on the number of workspaces that one invite grants. Each of these lists is sent as a whole: in a response, in a snapshot and in an event (ADR 0208). An event has a largest size (ADR 0011), and a list without a bound can pass it. The permission function reads every role of an account for each decision.

## Decision

- A workspace has at most 250 roles, the "everyone" role included.
- A member holds at most 64 roles of one workspace, without the "everyone" role.
- An account holds at most 64 server roles.
- A category and a channel hold at most 250 person rules each. A rule for a role is bounded by the number of roles of the workspace. The route that reads the rules of a place returns them as one list.
- An invite and an approval of a join request name at most 100 workspaces. A request with more gets `invalid_request`.
- A request that would pass one of the first three ceilings gets the error `limit_reached` with the status 409, and nothing is changed.
- The server counts in the transaction of the change (ADR 0009). Two requests that each stay below a ceiling and together pass it cannot both commit: PostgreSQL ends one of them with a serialization failure, the server runs it again, and it then gets `limit_reached`.
- The ceilings are enforced by this code and not by a constraint of the database. A check constraint cannot count rows, and a trigger that counts would need a lock of its own to be correct for writers at a lower isolation level, which the server does not have (ADR 0009). The invariants that one row can state stay constraints.
- The numbers are values of the server configuration in the code. No environment variable changes them.

Grant ceilings of person rules, category rules, sync and moves (ADR 0304):

- Nobody gives what they do not hold. The permissions that a change gives are the permissions added to an allow set and the permissions removed from a deny set.
- A rule of a channel: the caller must hold what the change gives in that channel.
- A rule of a category: the caller must hold what the change gives in the category and in every synced channel of it.
- Syncing a channel again: for each subject, what the rules of the category give compared with the rules that the channel uses now. The caller must hold all of it in the channel, and must outrank each person whose rule the sync changes or be that person.
- Moving a synced channel into another category: the same check as a sync, against the rules of the destination category, before the move is written. A move that would give more is refused with `exceeds_own_permissions` and nothing changes, also for the whole request of `PATCH /api/v1/workspaces/{workspace_id}/channels/positions`.
- The workspace permissions of a member: `GET`, `PUT` and `PATCH` on `/api/v1/workspaces/{workspace_id}/members/{account_id}/permissions` (`PersonPermissions`). They require manage roles in the workspace, and the caller must outrank the member (ADR 0309); the own account is refused. The caller must hold in the workspace what the change gives, and in each channel the permissions that the change adds to what the member can use there. A channel where a rule still denies the permission to the member does not count. The change sends `workspace_person_permissions_set` to the holders of manage roles in the workspace, writes the audit entry `workspace_person_permissions_set`, and sends `permissions_changed` to the member.
- The server permissions of an account: `GET`, `PUT` and `PATCH` on `/api/v1/accounts/{account_id}/server-permissions` (`ServerPersonPermissions`, with `SetServerPersonPermissions` and `ChangeServerPersonPermission`). They require the administrator permission, and the caller must outrank the account. Only the server owner adds a deny of the administrator permission. The account of the server owner takes no rule; a change of it gets `invalid_request`. The change sends `server_person_permissions_set` to administrators, writes the audit entry `server_person_permissions_set`, and sends `permissions_changed` to the account.
- A change that gives a person a permission needs an active account, as an assignment of a role does (ADR 0113). A change that only takes permissions away is also accepted for an inactive account.

## Consequences

- The largest events of the server base stay far below the size that an event may have: the server roles of an account, the roles of a member, an invite with its workspaces, and the overrides that the event of a deleted channel keeps.
- A workspace that needs more than 250 roles, or a member who needs more than 64, needs a change of this record.
- A program that writes to the database directly, at another isolation level, can pass a ceiling. ADR 0009 states the same for every rule of the server.
- ADR 0309 limits the creation of server roles to 250. The number of workspaces, of channels and of invites has no ceiling. None of them is sent as one list in an event.
- A removal of an account records one event per workspace that the account was a member of, and a join one per granted workspace. Their number is bounded by the workspaces of the invite or by the memberships of the account.
