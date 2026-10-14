# 0311. Role action flags and single-permission changes

## Status

Accepted

## Context

A client cannot derive an unban decision from server roles alone: restored workspace roles
also count. Channel overrides likewise change the grant ceiling when assigning, removing
or deleting a workspace role. Replacing the whole permission list loses concurrent edits
to different permissions.

## Decision

Account-list rows include `may_unban`. It is true only for a banned account that the
caller may unban through the current moderation and restored-role rank checks. Access
tokens always receive false because they cannot unban (ADR 0213).

Server-role and workspace-role list rows include `may_assign`, `may_remove` and
`may_delete`. The shared checks used by the corresponding requests compute these flags:
management permission, strict role rank, and the grant ceiling, including channel allows
on assignment and lifted channel denies on removal or deletion. Everyone cannot be
assigned, removed or deleted. Access tokens cannot assign or remove; deletion also
requires their manage_structure action. Flags describe authority over the role; target
membership, active standing and assignment-count ceilings remain request-time conditions.

Flags reveal no channel identifier, override, restored role or reason for a refusal. The
list's existing visibility check runs first. They describe the caller at that transaction,
are absent from shared role events and structure exports, and are not authority for a later
request. A caller without role-management permission gets false flags without loading channel checks.
Ranks, channels and overrides are read once per managed list; pure decision functions
share those facts across rows and are also used by mutations. Statement counts do not
grow with the number of listed roles or banned accounts. A client refreshes lists after permission changes and handles later refusals.

`PATCH /api/v1/server-roles/{role_id}/permissions` and `PATCH
/api/v1/workspaces/{workspace_id}/roles/{role_id}/permissions` accept one `permission` and
an `enabled` boolean. They read the role under its update lock, change that bit, and reuse
the whole-list update's rank, ceiling, audit and event logic. Serializable retries reread
the current set. Concurrent changes to different bits therefore both survive. Access
tokens require manage_structure, as for whole-list updates. A request that leaves the
permissions unchanged still checks permission, rank and ceiling, but records no event or
audit entry and returns a null receipt.

Role reorder accepts every positive signed-32-bit position. Zero remains reserved for the
workspace everyone role. When the highest position is i32::MAX, creation compacts the
existing roles to 2, 3, 4, ... in their current order and inserts the new role at 1. It
publishes the changed positions in the existing bounded batches. Relative rank is
preserved and creation does not require a privileged caller to repair the ordering.
Otherwise creation retains the ordinary shift, so a shift beyond 1,000,000 remains movable. The 250-role reorder bound and strict old/new rank checks do not change.

## Consequences

Old readers ignore the additive list fields. New readers use flags instead of guessing
from incomplete role information, and use permission requests for individual toggles.
The original role and account description types remain usable for shared events and
existing readers; caller-specific list descriptions flatten those descriptions on the wire.
