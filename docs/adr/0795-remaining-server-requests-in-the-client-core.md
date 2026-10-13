# 0795. The remaining server requests in the client core

## Status

Accepted

## Context

The server added the last things that the client asked for: a reason at a ban and at a removal, the removal of a member from one workspace with the event `workspace_member_removed` for the removed account (ADR 0106), a request that changes one permission of an override (ADR 0304), filters of the audit log that the server applies (ADR 0305), the name of the creator in an invite and the event `invite_changed` (ADR 0111), the description of the avatar in a member list and in a call participant (ADR 0119), the error code `last_device` (ADR 0209), the logo of the server with the event `server_logo_changed` (ADR 0101), and the hide setting and the admission state of a call (ADR 0517). The client core holds the state of a server and the routes of the API (ADR 0719, ADR 0760).

## Decision

Routes:

- A ban and a removal keep their functions without a reason and get one with a reason each. A reason is sent without the space around it. A request without a reason, or with a reason of spaces only, has no body, as before the reason existed.
- One function removes a member from one workspace, with an optional reason.
- One function changes one permission of the override of one role in one channel to `allow`, `deny` or `inherit`. The function that sets the two whole sets stays for the removal of an override.
- The two functions that read the audit log take the filter query of the protocol: action, actor, target, start and end of a period, the entry to continue after, and the size of a page. They replace the functions that took a page only.
- Three functions read, set and remove the logo with a session. A fourth reads the logo without a session, from the route of the chosen API version, and before the first contact of a run from the path that the server book holds. A server without a logo answers `not_found`, which these functions return as "no logo". An address that answered as another server gets no request for a logo (ADR 0714). The image of a logo is checked as the image of an avatar is: its size and its first bytes.

State:

- `ServerModel.logo` is "not stated" after a snapshot, because the snapshot does not carry the logo, and "stated" with the description or with none after `server_logo_changed`. A reader that shows the logo reads the image when the description differs from the image that it holds, and again after a reload of the state, because events between two states are not delivered.
- `invite_changed` replaces the invite in the loaded list of invites, as `invite_created` adds one.
- `account_profile_updated` sets the description of the avatar of the loaded members with their name.
- `workspace_member_removed` removes the member from a loaded member list. For the own account the event changes nothing else: the server records `permissions_changed` in the same change, and the snapshot that follows it holds the workspace no longer.
- `workspace_rank_of` states where a member of a workspace stands for an action on its membership, by the function of `junto-authz` that the server uses: the owner, then the highest position of a server role that gives "administrator", then the highest position of a role of the workspace. It is unknown while the server roles of an account that holds some are not loaded.

Calls:

- The new fields of `call_started`, of the own call and of the answer of `GET /call` are read by the protocol and are not used by the state. The call state of the client is changed by another record.

`junto-cli`:

- Commands for the reason, the removal from a workspace, one permission of an override, the filters of the audit log, the logo, and the roles of both levels: list, create, rename, permissions, delete, give, take and move. A move reads the roles and sends one request in which the roles between the old and the new place take the position of their neighbour, so the set of positions stays the same.

## Consequences

- A caller of the two functions of the audit log states a filter query. A query without a filter reads as before.
- A reader of the logo costs one request of the image after each reload of the state of a server with a logo.
- The reason of a ban or of a removal is in no event and in no state of the client core. A reader finds it in the audit entry.
- A page that states the rank rule of a removal from a workspace needs the list of the accounts and, for an administrator who is not the owner, the server roles.
- The state does not tell whether the server has seen the device of the account in the room of a call, and it takes the hide setting of a call from its channel, as before.
