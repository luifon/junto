# 0309. Role order

## Status

Accepted; replaces the absence of role order in ADR 0304.

## Context

Permission sets say what a member can do, but do not protect another moderator from
removal or protect a role from a lower role manager. Positions provide that boundary
without treating each small permission as protection from every moderator.

## Decision

A role-order check requires the existing permission and a strictly higher highest role;
the owner stands above all roles and server actions use server-role positions.
Inside a workspace, administrators stand above workspace roles and compare by their
highest administrator-granting server role; everyone else uses only that workspace's
roles, and other server roles contribute no position.

- A role has a nonnegative integer `position`; larger is higher. Positions are unique
  among server roles and within each workspace. The workspace everyone role is fixed
  at zero. No assigned role means zero at server level and everyone at workspace level.
  A server role without administrator contributes no workspace position. Equal highest
  roles refuse role-order checks. Permissions still combine as ADR 0301 decides, irrespective of position.
- Removal, ban and unban check role order. Unban restores roles in Junto (ADR 0106),
  unlike Discord: it therefore requires the actor above the restored server position
  and above the restored position in every workspace with assigned roles. The owner
  can always unban; existing owner/self and access-token restrictions still apply.
- Giving and taking roles compare the role with the actor, not the recipient's highest
  role, as Discord's role API does. This includes changes of the actor's own lower roles
  and of lower roles of a higher member or the owner. A role at or above the actor is
  refused even when the recipient is the actor. Existing permission ceilings and the
  token prohibition of ADR 0213 apply.
- A created role and each directly edited, deleted or reordered role must remain
  strictly below the actor. A new role is inserted at the bottom above everyone; existing
  roles move up one without changing their relative order, in one update. A creation
  writes one audit entry and, under the 250-role ceiling, at most three events:
  positions-shifted events of at most 128 positions each before the created event. Each
  carries absolute positions of existing non-everyone roles, so applying it after a fresh
  role-list read cannot increment positions twice. It emits no permissions notice because
  relative authority has not changed. A role manager above everyone can edit it but cannot
  assign, delete or move it. Channel overrides do
  not use role order, as Discord does; ADR 0304's grant ceiling still applies.
- Voice microphone blocks and their removal, sound blocks and their removal, disconnect,
  moves (both channels and completion), meeting start/end and presenter unblocking,
  lowering another hand, entry decisions and invitations do not use role order, as
  Discord voice permissions do not. Junto's presenter and entry rules remain those of
  ADRs 0512, 0514 and 0516. Personal microphone/sound settings, personal conversation
  mute, leaving, and personal hand state likewise do not compare roles.
- Deleting another person's message, pinning, reactions, moderation of recordings and
  transcripts, personal blocks, revocation of another creator's invite, denial of a join
  request, admission by invite or approval, adding an existing
  account to a workspace, and revoking access tokens do not compare roles. They retain
  their existing permissions and visibility limits. Editing another person's message or
  profile has no route or permission. Account recovery and owner transfer keep their
  owner-only rules. Workspace-only removal requires administrator, protects the owner and self, and compares the target's workspace rank strictly; it is ranked, unlike admission.
- These exemptions preserve Discord's action-specific role order; role order is not a
  general immunity from moderation. Junto differs by having two role levels and by
  retaining its existing permission-grant ceilings, permanent-message rules and token
  restrictions. Workspace roles isolate each workspace's authority; grant ceilings prevent
  a manager creating permissions they lack; permanent-message rules preserve retention;
  token restrictions prevent restoring account access through a restricted credential.
  Meeting, entry and admission actions keep their specific consent/visibility decisions
  because their Junto workflows differ from Discord. No workspace role can give authority
  over an administrator-granting server role.
- An access token uses its creator's current highest role. The owner's token has the
  owner's position for role-order checks, but still has no owner-only permission (ADR 0210).
  Every request reads positions in its transaction; reordered roles affect the next
  request. In-progress voice moves are exempt from role order and retain their permission recheck.
- The migration puts roles granting administrator or ban_members above ordinary server
  roles, and roles granting manage_roles above ordinary workspace roles. Each group
  orders by creation time and identifier. These are the role-dependent permissions at
  each level; manage_members is a token action, not an assignable permission. Everyone
  stays zero; the owner can rearrange positions among moderators after migration.
- The migration renews the event epoch, so clients discard pre-upgrade cached roles and
  read positions from a fresh snapshot and role lists. Historic events may omit position.
- New servers have at most 250 server roles, matching the workspace ceiling. Existing
  servers above that ceiling retain their roles and can delete or reorder them, but cannot
  create more until below it. A reorder changes at most 250 named roles, with positive
  distinct positions no larger than 1,000,000, in one transaction; unnamed roles keep their position, and a
  collision with one is `invalid_request`. Both old and new positions must be below the
  actor's highest role, evaluated before the reorder; a reorder cannot raise the actor's highest role.
- `PATCH /api/v1/server-roles/positions` and `PATCH
  /api/v1/workspaces/{workspace_id}/roles/positions` accept a `positions` list of role
  identifiers and positions. They belong to `manage_structure`. Existing role-updated
  events carry each changed position; each moved role gets a role-position-changed audit
  entry and its holders get `permissions_changed`. An unchanged position writes nothing.
- Snapshots and role lists include position. Server-role lists and events reach holders
  of ban_members as well as administrators. Workspace-role lists already reach every
  workspace member, including manage_roles holders; a manage_members token uses its
  creator's permission and gains read access to server-role positions.
- Structure format 2 expresses relative order using optional positive position keys.
  Format 1 without role positions remains readable. The planner sorts named positioned
  roles by their keys, assigns them the existing slots they occupy after creations and
  deletions, and leaves unnamed or unpositioned roles in place. A new unpositioned role
  stays at the bottom. Numeric gaps and shifts alone produce no reorder; an old export
  survives later client creations, and a second apply changes nothing. Plans name each
  moved role and its resulting position. A reordered own highest role is still refused.
- Taking an unknown role returns not_found, after the existing visibility checks.
  Revoking a higher person's access token stays exempt: it removes access, grants none,
  requires administrator and leaves owner-only account recovery intact.
- Role-order refusals use `forbidden` with a fixed message and no target detail. Existing
  visibility checks run before role lookup; hidden workspaces still return `not_found`.

## Consequences

An administrator cannot edit its own highest role or remove an equal administrator.
Giving a lower role to a higher member is allowed; giving one's highest role to anyone
is not. The owner remains able to repair any arrangement. Role order is independent of the
permission evaluation and is decided by one function in `junto-authz`.
