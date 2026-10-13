# 0761. The administration in the window

## Status

Accepted

## Context

A person with the permissions manages workspaces, categories, channels, overrides, members, invites, join requests, access tokens and exports, and reads the audit log (ADR 0903, step 5). The approved design has no view for any of these. ADR 0703 fixes three columns, one job per area, and the components of ADR 0731. The client core holds the collections of the administration on demand (ADR 0760). The server decides every request, takes no reason for a removal or a ban, has no override for a single member (ADR 0301), and filters the audit log by a page position only (ADR 0305). The screen that creates, edits, orders and assigns roles waits for the role order of the server and is not part of this record.

## Decision

One layer:

- The administration is one layer above the window: a list of pages at the left and one page at the right. The three columns of the window stay as the design has them. The layer holds the keyboard focus, and Escape, its close button and a click outside close it.
- The menu of the server opens it. A button in the header of a channel opens the same layer with the settings of that channel.
- The pages of a server are in three groups. Server: Workspaces, People, Invites, Join requests, Access tokens, Server roles, Audit log. The shown workspace: Channels, Roles, Archive and retention, Members, Recordings. The account: Exports. ADR 0791 decides the two pages of the roles, and ADR 0784 the page of the recordings. The settings of a channel have Overview, Permissions, and Archive and deletion.
- A page that the person may not use is absent. On a page that the person sees, a control that the person may not use is disabled, with the reason as a sentence or a tooltip. What is possible follows the server permissions of the account and the lists of workspace permissions that the server states for the workspace and for a channel (ADR 0790). The server decides each request.
- A form and a question are drawn in the row that they belong to, in place of the actions of that row. No second layer opens above the first. A deletion, an archiving, a removal and a ban are questions that state what follows.
- Each request has the states sending and refused. A refusal is a sentence at the control that asked, chosen by the error code of the server. A request that the server did leaves no state of its own: the change arrives with the state of the server. Only the answer to the newest request of the open page changes what the page tells and holds. An answer to an earlier request, and an answer that arrives after the page changed or the layer closed, is dropped with what it carries, a code or a token included; the change that its request made arrives with the state of the server.
- A form whose request is on its way is not sent a second time, by its button or by Enter.
- The layer belongs to the account that its server showed when it opened. When that server shows another account, or none, the layer closes and keeps nothing of the account before.
- The window holds open the collections that the open page shows and releases them when the page changes. A collection that is on its way or was not read is stated in place of its rows, with "Try again" after a failure.

Pages:

- Workspaces: the list with the default workspace and the membership of the account, a new workspace, a new name, the deletion, and an invite code that enters further workspaces (ADR 0115).
- Channels: the categories and the channels of the shown workspace in the order of the channel list; a new category and a new channel in a category; a new name, the order, the move of a channel into a category, and the deletion of a category. A move by one place exchanges the positions of two neighbours; when two entries share a position, every entry of the group gets its place as its position.
- Channel settings, Overview: the name, the category, "permanent messages", "private channel", and for a voice channel the user limit and "hide who is inside". "Private channel" is the deny of "view channel" in the override of the "everyone" role (ADR 0304). The lock and meeting mode stay controls of a running call (ADR 0739). An archived channel shows its settings and takes no change.
- Channel settings, Permissions: per role of the workspace one choice per permission: allow, default, deny. A choice that the grant rule of ADR 0304 refuses is disabled with its reason: to allow a permission, or to lift a deny of it, the person must hold it in the channel. Overrides exist per role only.
- Channel settings, Archive and deletion: archive or restore, the export, and the deletion. The deletion of a channel with permanent messages is offered to the server owner only (ADR 0402).
- Archive and retention: the archived channels of the workspace (ADR 0762), and for an administrator the archive period, the retention of files and of recordings of the workspace, and the retention of the files of direct messages.
- People: the accounts of the server with their status and the names of their server roles; removal, ban and unban with a question; a filter for the banned accounts. Members: the members of the shown workspace with the names of their roles, and for an administrator the accounts that can be added. The roles of an account are given and taken in these two lists, and role order decides which account takes a removal or a ban (ADR 0791).
- Invites: the list with creator, expiry, uses, workspaces and state; a new invite with expiry, uses and the workspaces that the person may grant (ADR 0111); the revocation. Join requests: the list, the approval with workspaces, and the denial.
- Access tokens: the list, a new token with a name, actions and an expiry of at most 366 days (ADR 0210), and the revocation. An administrator has the page among the pages of the server, with the tokens of every account. Every other account has it among the pages of the account, with its own tokens and without the creation: the server lets an account revoke its own tokens after it lost the administrator permission.
- Exports: the exports of the account with state, size and expiry; the download into the download directory with its progress; the removal. An export is asked for in the settings of a channel and in the header of a direct conversation (ADR 0411).
- Audit log: the log of the server and the log of the shown workspace, read in pages of 50 with "Load older"; one readable line per action; an action that this build does not know is shown with its time and its actor (ADR 0209). The filters by kind of action and by person work on the entries that are loaded.

Secrets:

- The code of a new invite and the token of a new access token are held by the open page only, shown once with "Copy", and gone from the state of the application when the person puts them away, changes the page or closes the layer. Neither is the accessibility label of its element. The copies that the drawing of a text makes are freed with the frame and are not erased, and a copy on the clipboard stays until the person replaces it.
- The file of an export is written beside its final name and renamed at the end, under a name that no file has. A body with more bytes than the export states is refused before the excess is written. A body that ends early is asked for again from the byte reached, with a wait before each further request, at most 5 times in a row without progress. Progress is 64 KiB beyond the furthest byte reached so far, or the end of the file, so a server that gives single bytes ends the save and an honest download of any size that is cut often ends whole. A save goes on when the layer closes. It is stopped, and its unfinished file removed, when its server leaves the client or shows another account.

## Consequences

- Every screen of the administration is designed in this record and not in the approved design.
- A person without any permission still has the layer: the workspaces, the invite code, the archived channels, the members, the own access tokens and the own exports.
- A removal and a ban carry no reason, because the server stores none. The audit entry is the record.
- A filter of the audit log does not find an entry that is not loaded yet. The page says how far the loaded entries reach.
- A move of a channel sends two requests, or one request for each channel of its group when positions are equal. A failure between them leaves the order of the first requests.
- The roles are created, changed and ordered on the pages of ADR 0791.
- A person who holds "manage channel" through an override of one channel has the pages of the settings of that channel (ADR 0790).
