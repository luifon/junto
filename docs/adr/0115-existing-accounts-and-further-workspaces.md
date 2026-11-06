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
- An account that is not active (banned, removed, inactive or deleted, ADR 0121) is refused with `conflict`.

Redeeming in the desktop client:

- The server menu ("Join workspaces with an invite") and the hover menu of a server in the collapsed server book (the same item) open the redemption of the account of that server: the dialog of the server with the field "Invite code" and "Join invited workspaces".
- An invite link of a server of the server book opens the same redemption with its code typed (ADR 0732). "Add server" with the address of a server of the server book opens the dialog of that server in place of a refusal: the redemption for an account, the ways in of ADR 0732 for a server without an account. A `junto://` link pasted into "Add server" opens as the link does, and the code of an invite link that names the server under another of its addresses is typed into the dialog.
- After a redemption the dialog names the added workspaces ("Your account is now a member of “Lab” and “Studio”."), with a count for the workspaces whose names have not arrived yet, and closes when the person presses "Close". The `conflict` of an invite that adds nothing is stated as "Your account is already a member of every workspace of this invite. The invite was not used."
- Administration → "Workspaces" keeps "Enter an invite code" for a person who is already there.
- Window tests: `workspace_invites::a_member_joins_further_workspaces_from_the_menus_of_the_server` and `workspace_invites::add_server_with_a_saved_server_redeems_into_its_account`; `links::an_existing_account_confirms_an_invite_into_another_workspace` for the link.

## Consequences

- One invite serves new people and existing accounts.
- A person with "create invites" can let an existing account into the workspaces that the person is a member of, and no others (ADR 0304).
- A direct addition needs no invite and no action by the added account.
- Removing an account from one workspace only is not part of this record.
