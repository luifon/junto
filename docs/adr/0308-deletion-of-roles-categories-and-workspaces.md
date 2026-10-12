# 0308. Deletion of roles, categories and workspaces

## Status

Accepted

## Context

The API created and changed server roles, workspace roles, categories and workspaces, and deleted none of them (ADR 0211). A role decides permissions, so its deletion can take a permission from its holders and, when an override of the role denies a permission, give one. A person who manages channels does not see every channel of a category. A workspace holds members, roles, channels, files and recordings, and audit entries refer to it (ADR 0305). An event is decided for each recipient when it is delivered (ADR 0011): after the deletion of a role nobody holds it, and after the deletion of a workspace nobody is a member of it.

## Decision

Workspace role, `DELETE /api/v1/workspaces/{workspace_id}/roles/{role_id}`:

- Needs `manage_roles` in the workspace. The "everyone" role gets `conflict`.
- The deletion removes the overrides of the role. An override that denies a permission is removed with it, which can add that permission to a holder. So the caller must hold, in each channel, the permissions that an override of the role denies there, and gets `exceeds_own_permissions` otherwise (the grant rule of ADR 0304). A caller who cannot see such a channel holds nothing in it.
- The role, its assignments and its overrides are deleted in one transaction. The identifier under which a structure file created the role is free again.
- Events: `workspace_role_deleted` for the people who see the workspace, and `permissions_changed` for the accounts that held the role. A client removes the role from the members and from the overrides that it holds.
- Audit entry `workspace_role_deleted` about the workspace, with the name and the permissions of the role and the number of holders.

Server role, `DELETE /api/v1/server-roles/{role_id}`:

- Needs the administrator permission. The role and its assignments are deleted.
- Events: `server_role_deleted` for the administrators, and `permissions_changed` for the accounts that held the role. Audit entry `server_role_deleted` about the server.

Category, `DELETE /api/v1/workspaces/{workspace_id}/categories/{category_id}`:

- Needs `manage_channel` in the workspace.
- The channels of the category stay and have no category. A refusal for a category that holds channels would tell the caller of channels that the caller cannot see.
- Event `category_deleted` for the people who see the workspace: a client clears the category of its channels. Audit entry `category_deleted` about the workspace.

Workspace, `DELETE /api/v1/workspaces/{workspace_id}`:

- Needs the administrator permission. When a channel of the workspace contains permanent messages, only the server owner deletes it (the limit of ADR 0402), and an access token of the owner does not (ADR 0210).
- The default workspace gets `conflict`: an invite that names no workspace grants it (ADR 0102).
- One transaction removes the members, the roles with their assignments and overrides, the categories, the glossary and the settings of the workspace, marks every channel as deleted and marks the workspace as deleted. The row of the workspace stays, because audit entries refer to it. The accounts stay.
- The content of the channels is removed afterwards in bounded transactions (ADR 0017). The transaction of the request changes rows of the structure only: their number does not grow with the messages, the files or the recordings.
- Event `workspace_deleted` for the accounts that were members and for the administrators. Audit entry `workspace_deleted` about the server.
- The audit entries of a deleted workspace are read by the administrators only, like the entries of a deleted channel (ADR 0303).
- An invite keeps naming a deleted workspace. A use of the invite grants the workspaces that exist. An invite whose workspaces were all deleted gets `invite_revoked`. An approval of a join request that names a deleted workspace gets `not_found`.

Events for accounts that a rule no longer names:

- The transaction of a deletion stores, with the event, the accounts that it is for: the holders of the deleted role, or the members of the deleted workspace (table `event_receivers`). The rows are written by one statement and are deleted with the event (ADR 0207).
- The delivery decides a listed event with the permission function: a listed account receives the notice of a role; the deletion of a workspace is decided as "see the workspace" with the listed membership, so an administrator who was no member receives it too.

Access tokens:

- The four routes belong to the action `manage_structure` (ADR 0210).

## Consequences

- A person with `manage_roles` cannot remove a deny that the person could not remove through the override route.
- The members of a deleted workspace who are in no other workspace keep their account and see no workspace. Two accounts whose only common workspace was deleted no longer exchange direct messages (ADR 0307).
- The name of a deleted workspace stays stored and is shown in audit entries only.
- The deletion of a workspace records no event and no audit entry for its single channels, roles and members.
- A category is deleted by a person who does not see every channel of it. Those channels lose their category.
- The identifier of a deleted workspace and of its channels stays in use for structure files (ADR 0211).

Role order and its action-specific exemptions are decided in ADR 0309.
