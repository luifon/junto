# 0791. Role order in the client, and the pages of the roles

## Status

Accepted

## Context

A role has a position, and a person changes, moves, gives and takes only the roles below the highest role that the person holds. Removal, ban and unban need the person above the account in the order of the server roles (ADR 0309). The server states a refusal of role order as `forbidden` without a detail. The roles of a workspace are in the snapshot with their positions; the server roles are a collection that an administrator and a holder of "ban members" read (ADR 0309, ADR 0760). The server tells a new position with the event of the role, and the shift that a new role causes with `server_role_positions_shifted` and `workspace_role_positions_shifted`, which carry absolute positions. The approved design has no view for roles. ADR 0761 left the roles screen out. ADR 0702 requires a keyboard path for every drag action, and a disabled control is no tab stop (ADR 0731).

## Decision

Order in the state:

- The roles of a workspace are held with the "everyone" role first and then from the highest position to the lowest. The server roles are held from the highest position to the lowest.
- An event of a role puts the role at its position. A shift sets the stated positions on the roles that the state holds; a second delivery changes nothing.
- The client core has two routes for a reorder, one for each level. A reorder names roles and positions, and the roles exchange the positions that they hold among themselves.

Where an account stands:

- The client core decides where the account stands with `role_rank` and `outranks` of `junto-authz`, the functions that the server uses. It gives them the positions that the state holds.
- On the server: the owner, or the highest server role of the account. In a workspace: the owner; an administrator at its highest role that gives the administrator permission, above every role of the workspace; every other account at its highest role of the workspace.
- A page of the roles whose person stands at an unknown place shows the read of the roles in place of its rows, and takes no new role.
- A position that the state does not hold makes the answer unknown: the server roles while their collection is not loaded, and a role that an account holds and the loaded list does not name. A role that is not known is not taken as no role. An account without a server role stands at zero without the collection.
- This is the one comparison of roles in the client. It decides no permission (ADR 0790).

The pages:

- The administration has the page "Server roles" for an administrator, and the page "Roles" of the shown workspace for a holder of "manage roles". A person who cannot manage the roles of a level does not have its page.
- A page states where the person stands: the owner, an administrator, the name of the highest role, or no role, with what follows from it.
- The roles are listed from the highest to the lowest. The "everyone" role is the last row.
- A row has the actions "Up", "Down", "Rename", "Permissions" and "Delete". A form and a question are drawn in the row (ADR 0761).
- A role at or above the person is locked: its row carries a mark and a sentence that states it, and every action that changes the role is disabled. The sentence is drawn in the row and not as a tooltip, so a person who uses the keyboard reads it. "Permissions" stays usable and shows the switches of the locked role as disabled: the person reads what the role holds.
- A form or a question of a role that becomes locked while it is open is put away, and the row shows the role as locked.
- A new role has no permission and enters at the bottom. The form is disabled, with its reason, for a person who stands at zero and at the ceiling of 250 roles (ADR 0306, ADR 0309).
- "Permissions" shows one switch for each permission of the level. A change of a switch is sent at once, with the whole list of the role as the state holds it. The switches of that role take no further change until the server answered and the state shows the change, until the request failed, or until the role is gone: a second change that was built on the list of before would undo the first. The state shows the change when it holds the permission as the change set it, or when its list changed after the answer. The application keeps this for each role by itself, whichever page or role the person looks at meanwhile and also while the layer is closed; another account on the server ends it. A switch that would give a permission that the person does not hold in the workspace is disabled, with the sentence under it (ADR 0304). A switch that takes a permission is never disabled for that reason. Every server permission can be given by a person who has the page.
- The deletion is a question that states what the server removes (ADR 0308). The client does not know the overrides of the role in every channel, so the refusal `exceeds_own_permissions` is a sentence at the row.
- The "everyone" role takes a change of its permissions by a person who holds a role. It has no new name, no deletion and no move.

Order by drag and by keyboard:

- A role is moved by its handle onto the row of another role and takes that place. "Up" and "Down" move it one place and are the keyboard path.
- A move is one request. It names the role and every role between its old place and its new one, each with the position of its new place, so that the request uses only positions of the named roles.
- A place is offered only when it and every place between hold roles below the person and none is the "everyone" role. A row that is no such place takes no drop and shows no drop mark. A locked role has no handle.
- A move is offered only while the positions of the roles that it names are in order in the state: they fall from one role to the next, each lies between 1 and 1,000,000, no other role holds one of them, and the move names at most 250 roles (ADR 0309). The positions are out of order for a moment while a reorder of another person arrives, one event for each role; the buttons state it then. A position outside the range is stated as that: the window exchanges positions that the roles hold and gives no new ones.

Roles of an account:

- The page "People" has the action "Roles" for an administrator and an active account, and the page "Members" has it for a holder of "manage roles". It shows one switch for each role of the level under the row.
- A switch is disabled with its reason in the row: the role is at or above the person; giving the role would give a permission that the person does not hold; the account holds 64 roles. Taking a role that the account holds is disabled only by role order.
- The grant rule for the overrides of a role in single channels (ADR 0304) is not known to the client. Its refusal is a sentence under the switches.

Removal, ban and unban:

- A row of "People" compares where the account stands on the server with where the person stands. An account at or above the person takes no removal, no ban and no unban: the three actions are disabled, and the row states the reason as a sentence. A question about an account that comes to stand at or above the person while the question is open is put away. While the order of the roles is read again, the question stays and its answer waits.
- The page "People" reads the server roles for every person who has it. While they are on their way the three actions are disabled with that reason. When their read failed, the actions are enabled and the server decides.
- An unban also compares the roles that return in every workspace (ADR 0309). The client does not hold the workspace roles of a banned account, so that refusal is a sentence after the request.

Refusals:

- A `forbidden` answer to a request about a role states that a role at or above the highest role of the person cannot be changed, moved, given or taken. A `forbidden` answer to a removal, a ban or an unban states the same for an account.
- An `invalid_request` answer to a move states that the order has changed.

## Consequences

- Every screen of roles is designed in this record and not in the approved design.
- A person sees which roles and which accounts the person can act on before a request is sent.
- A change of the order by another person reaches an open page as events, and the rows follow.
- The state of a row can be older than the server by the time of one event. The server decides, and its refusal is a sentence.
- A move over many places names every role between the two places, at most 250.
- The roles of a workspace cannot be created with permissions in one step: a new role gets them afterwards.
