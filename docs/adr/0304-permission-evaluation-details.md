# 0304. Details of the permission function: view permission, grant rule and stored sets

## Status

Accepted

## Context

ADR 0301 fixes the two levels of roles, the evaluation order, the administrator permission and its limits. An implementation needs rules that it leaves open: which permission makes a channel visible, what exactly "nobody grants a permission they do not hold" covers, which permission the actions without a named permission need, which workspaces an invite can grant, and how a set of permissions is stored.

## Decision

One function:

- The crate `junto-authz` has one function that takes the caller, a request and the roles and overrides that the request needs, and returns allow or a reason for the denial. Every route calls it. A route contains no comparison of roles or permissions.
- The function runs in three steps: the limits, the administrator permission, the roles. A request that a limit decides never reaches the second step.
- The server owner passes every check that the administrator permission passes.

View permission:

- The workspace permission "view channel" is added to the list of ADR 0301. A private channel is a channel whose override of the "everyone" role denies it.
- Every permission in a channel needs "view channel" in that channel as well.
- An absent channel reads the same workspace-role, membership, person-rule and effective-channel-rule statements as a hidden channel before returning `not_found`. An absent category likewise runs the category and child scope reads before the visibility decision. Database-statistics tests compare the statements of repeated requests; they do not assert identical elapsed time. Only the `not_found` result waits for the remaining reads; a database error of a read is returned at once, because PostgreSQL refuses every later statement of a failed transaction and a serialization failure must reach the retry of ADR 0009.
- The "everyone" role of a new workspace has "view channel", "lock room" and "run meeting mode".

Membership:

- A request inside a workspace is denied to an account that is not a member of it, unless the account has the administrator permission.

Grant rule:

- A change is checked against the permissions that it can add to any account:
  - creating a role: the permissions of the role;
  - editing a role: the permissions that the edit adds;
  - assigning a role: the permissions of the role, and for each channel the permissions that the channel overrides of the role allow there;
  - removing a role from a member, and deleting a role (ADR 0308): for each channel the permissions that the channel overrides of the role deny there, because the member gets them back;
  - changing a rule of a role or a person in a channel or a category: the permissions added to the allow set and the permissions removed from the deny set;
  - syncing a channel again, and moving a synced channel into another category: for each subject, what the new rules add compared with the rules that the channel uses now;
  - changing the workspace permissions of a member, and the server permissions of an account: ADR 0306.
- The caller must hold each of those permissions: in the workspace for the permissions of a role, in the channel for the permissions of an override. A caller who cannot see a channel holds nothing in it.
- Removing a permission from a role and adding a deny need only the managing permission. So does removing a role whose overrides deny nothing.
- The rule for the removal of a role is the same for every target: the caller's own account and another member.

Managing permissions:

- Workspace roles need "manage roles". Channels, categories and channel overrides need "manage channel".
- Server roles, renaming a workspace and adding an existing account to a workspace (ADR 0115) need the administrator permission, because ADR 0301 names no other permission for them.
- Removal, ban and unban need "ban members". The server owner and the caller's own account cannot be the target (ADR 0113).
- Creating an invite or approving a join request needs its server permission, and the caller must be a member of every workspace that it grants.
- The account that creates a workspace becomes its first member and gets no role in it.

Attachment uploads:

- Starting an upload, adding a chunk and completing it require attach files and at least one of send messages or send messages in threads. The destination thread is not chosen until the message is sent. Each phase rechecks the current permission; final binding rechecks attach files.
- A person who loses attach files may still inspect or cancel their pending upload while they can see its channel. This lets the person release an unused upload without restoring the write permission.

Messages and threads:

- Sending or editing a top-level channel message requires send messages. A thread reply requires send messages in threads instead, on both send and edit. Direct conversations retain their participant rules.
- The first reply additionally requires create threads. A stored reply or a retained thread writer record establishes an existing thread, including after its last reply was deleted. Following an empty root does not establish a thread.
- Adding a reaction requires add reactions. Removing one's existing reaction remains allowed.
- Adding an ordinary role mention on send or edit requires mention roles. The everyone role and the everyone mention require mention everyone. An unchanged mention retained by an edit requires no new grant and sends no new mention notice.

Message reads:

- Channel and thread history require read message history. A single channel message also requires it unless the message was created strictly after the current subscribed event connection began. The server records this time; requests cannot supply it.
- Message event audiences retain the original message creation time, including system lines and deleted replies whose rows are erased. An edit or deletion time must not make older content qualify as a live message. Deletion records its event in the same transaction before erasing the message row.
- Message change events use this creation-time rule when replay is counted, when events are delivered and when a request receipt is selected. A retained old event without creation metadata requires history permission. Current visibility and the saved readers of private system lines remain required.
- The live exception uses the latest subscribed connection for the requesting session and device. A replaced, unsubscribed or stopping connection grants no exception. Reconnecting establishes a new boundary. Current view channel permission is always required.
- Search results and unread thread lists include only channels with read message history. Scoped search, pins, thread history and a thread copy require that permission explicitly. An export checks both export channel and read message history when requested, while being built and during download.
- Saved notices, their pagination cursors, read updates and notification events require history permission or a message created after the current live connection began. The SQL applies this bound before pagination and counting. A snapshot retains live notices and the settings of visible channels without exposing old notices or old unread threads.
- Recordings and transcripts require read message history, including their files, corrections and management routes. Starting and stopping a recording retain the separate in-call and record calls requirements.
- An attachment download uses the same creation-time bound as its message, including when permission is checked again during a running download. A reconnect can therefore end a history-free download of an older message.
- Sending and editing return the written message without requiring read message history. Internal checks of a reply destination and updates of its root do not turn an authorized write into a history read. Direct conversations retain their participant and departure rules.
- A change of a message that is older than the current connection, in a channel that the recipient sees without read message history, reaches the connection as `channel_history_withdrawn` with only the workspace and the channel, at most once per change and channel, in live delivery and in replay; replay counting and request receipts count it in the same way. The event of the creation of an older message and a mention send nothing, because the connection cannot hold that message. Reason: a client kept deleted or edited old content, and no event could tell the change without an old message identifier. Tests: `history_permissions::withdrawn`.

Message edits:

- Editing a message is denied when the message is permanent, when the caller is not its author, and in a direct message when the caller is not a participant. These are limits. No permission lets a person edit the message of another person.

Stored sets:

- A set of permissions is stored as one integer in which each permission has a fixed bit. A test fixes the bit and the name of every permission.
- The database refuses an integer with a bit above the highest known one. A new permission needs a migration that widens that check.
- Migration `0039_person_and_category_permissions` stores server, workspace, category and channel person overrides, category role overrides, and whether a channel is synced. Composite foreign keys keep roles and members in the correct workspace. Sets are disjoint and nonempty; removing the last bit removes the row. Workspace sets use bits 0–30.
- Existing everyone roles gain the channel-use permissions that previously followed visibility, including ordinary role mentions and voice status. Existing manage-channel grants and overrides also grant or deny setting the call user limit. Existing channel overrides stay detached; channels without overrides start synced with their initially empty category. The event epoch changes so a client reads the new permission lists.
- A synced channel reads its category role and person overrides. Its first actual override change copies all of those rules into the channel and detaches it in the same transaction. An identical request keeps it synced. Deleting a category first detaches its children, preserving their permissions, then removes the category, and sends `channel_rules_synced` with `synced` false for each child that was synced.

Rules of categories and channels:

- A rule has a subject, which is a workspace role (`{"kind": "role", "role_id": ...}`) or one member of the workspace (`{"kind": "person", "account_id": ...}`), an allow set and a deny set of workspace permissions. Two empty sets mean that the subject has no rule; storing them removes the row.
- `GET /api/v1/channels/{channel_id}/rules` returns `ChannelRules`: the channel, its category, `synced`, and the rules that the channel uses. `synced` is true when the channel has a category and takes its rules; the rules are then the rules of the category. It requires manage channel in the channel.
- `PUT` and `PATCH` on `/api/v1/channels/{channel_id}/rules/roles/{role_id}` and on `/api/v1/channels/{channel_id}/rules/people/{account_id}` set both sets (`SetChannelOverride`) or one permission to `allow`, `deny` or `inherit` (`ChangeChannelOverride`), preserving every other bit. They return the `PermissionRule`. They require manage channel in the channel. The channel row is locked and the change is made in the serializable transaction; retries preserve concurrent changes of other permissions.
- `GET /api/v1/workspaces/{workspace_id}/categories/{category_id}/rules` returns `CategoryRules`, and `PUT` and `PATCH` on `.../rules/roles/{role_id}` and `.../rules/people/{account_id}` change the rules of the category in the same way. They require manage channel in the category, which is evaluated with the rules of the category. A change of the rules of a category reaches every synced channel of it at once, because a synced channel reads the rules of its category.
- The role of a role rule must be a role of the workspace of the place. The person of a person rule must be a member of that workspace. Otherwise the answer is `not_found`, the same as for an absent account. For a person rule the caller must outrank the person in the workspace (ADR 0309) or be the person.
- `POST /api/v1/channels/{channel_id}/rules/sync` removes the own rules of a channel, which then takes the rules of its category again, and returns `ChannelRules`. A channel without a category gets `invalid_request`. A synced channel is returned as it is and nothing is written.
- Moving a synced channel into another category makes it take the rules of the destination category. The own rules that it had are removed. Moving a synced channel out of its category to no category copies the rules of the category into the channel first, so that the move does not change who can see and use it; the channel is then not synced. Moving a channel that is not synced keeps its own rules. A channel without a category that never had rules counts as synced, so it takes the rules of the category that it moves into.
- A hidden channel or category, and an absent one, get `not_found` on every rule route. A caller who sees the place but does not hold manage channel gets `forbidden`.
- A request that leaves a rule as it is, and an override of a structure file (ADR 0212) that leaves it as it is, writes nothing: no audit entry, no system line and no event, and a synced channel stays synced. Reason: the `PUT` route and the structure file wrote an audit entry and a line for a request that changed nothing.
- Each change has an audit entry: `channel_override_set` for a role rule of a channel, `channel_person_rule_set`, `category_role_rule_set`, `category_person_rule_set`, and `channel_rules_synced` when a channel detaches, is synced again or moves into another category while synced. The entry of a channel names the channel; the entry of a category names the workspace and has the category identifier.
- Each change sends the event `permission_rule_set` with the workspace, the place (`{"kind": "channel", ...}` or `{"kind": "category", ...}`), the subject and both sets. Its audience is the people who hold manage channel in that channel or category, decided at delivery. `channel_rules_synced` has the audience of the channel. A client updates the rules that it holds from `permission_rule_set`, also for a synced channel of the category that the event names, and reads the rules of a channel again after `channel_rules_synced`.
- The people whom a change concerns get `permissions_changed`: the holders of the role of a role rule, the person of a person rule, and every member of the workspace after a sync or a move that changes the rules of a channel. The rooms of the calls of the changed channels are marked, so that the next admission check reads the new rules (ADR 0509).
- Access tokens (ADR 0210) reach the reading routes with `read_structure` and the role-rule routes and the sync route with `manage_structure`. No access token reaches a person-rule route.

Member access:

- `GET /api/v1/workspaces/{workspace_id}/members/{account_id}/access` returns, for each channel of the workspace that the caller can see, the permissions for which the permission function allows a request of the member there (`ChannelPermissions`). A channel that the member cannot see is listed with no permission. A channel that the caller cannot see is not listed, so the answer does not tell that it exists. It requires manage roles in the workspace; an account that is not a member gets `not_found`.
- `PATCH /api/v1/workspaces/{workspace_id}/channels/positions` takes 1 to 200 moves (the constant `MOST_CHANNEL_MOVES` of the protocol, which the window reads to offer no larger move, ADR 0761), each naming a channel of the workspace once, the category and the position where the client read it, and its new category and position. One serializable transaction first checks every channel: it exists and the caller holds view channel in it, both answered with `not_found`, so that the answer does not tell whether a channel that the caller does not see exists or in which workspace it is; it belongs to the workspace; the caller holds manage channel in it; it is not archived; and its stored category and position equal those of the move; then it applies each move with the function of the route of one channel, with the same audit entry and `channel_updated` event. A failed check ends the request, `conflict` for a channel that is no longer where the move read it, and nothing changes. A move whose new category and position equal those where the client read it is a check only: the caller needs view channel in it and not manage channel, and it is not written, so it has no audit entry and no event, also in a request of checks only. A client sends the channels next to the destination that it read as such checks: the place of a moved channel is computed from its neighbours, and a move that names only the channels that change would be applied after another person moved a neighbour, to a place that the client did not choose. A change of the order that touches several channels is therefore applied whole or not at all, and only to the order that the client computed it from. Tests: `structure::channels_are_moved_together_or_not_at_all`, `structure::a_move_is_refused_when_a_neighbour_that_it_checks_has_moved`.

## Consequences

- One test suite of table tests and property tests covers the rule for every route.
- A person with "manage roles" cannot use a role to enter a channel: assigning a role whose override allows "view channel" in a channel that the person cannot see is denied, and so is removing a role whose override denies "view channel" there, from the own account or from another member.
- The roles and overrides that a decision uses are read in the transaction of the change (ADR 0009), so a permission that was revoked before the change commits is not used.
- A person with "manage channel" who denies a permission to a role that the person holds loses that permission in the channel and cannot remove the deny again. An administrator can.
- Role changes and server moderation also obey the order of ADR 0309. A person with "manage roles" changes only roles below their highest role, with the permission-grant ceiling above.
- A person with "create workspace" and no administrator permission gets an empty workspace that an administrator has to give roles to.
- A person with manage channel cannot use a person rule, a role rule, a sync or a move to give anyone, the own account included, a permission that the person does not hold, or a channel that the person cannot see.
- A category is listed only when a person sees a channel in it or can manage the category. Its management permission is evaluated with its role and person overrides. Renaming or deleting a hidden category, and creating or moving a channel into one, answers as for a missing category. A person who can manage one category can create channels there without workspace-wide management permission.
- Category creation and rename events use the current category visibility at delivery and replay. A category deletion sends a workspace permission refresh without its identifier, so a person who never saw it learns no hidden name or identifier.
- The number of a permission bit cannot be reused for another permission.

Mention events retain the message creation time, including mentions added by editing. Delivery and replay require read message history unless that creation time is after the verified current live connection start; adding a mention does not make an older message readable. Legacy mention audiences without a creation time require history.
