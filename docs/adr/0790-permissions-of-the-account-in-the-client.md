# 0790. The permissions of the account in the client: the lists of the server

## Status

Accepted; replaces the permission parts of ADR 0719, ADR 0760 and ADR 0761.

## Context

The client offered an action when the roles of the account in the workspace gave its permission (ADR 0719). It did not know the overrides of a channel, which only a person with "manage channel" reads, so it offered an action that a deny override refuses and did not offer one that an allow override grants. For a person who read the overrides, the client core computed the permissions of a channel with the permission function of the server (ADR 0760). The snapshot now states the workspace permissions of the account for each workspace, and for each channel in which the account holds another list (ADR 0310). An archived channel is in no snapshot, and one route states the list of the caller in one channel. `permissions_changed` stays the one event for a change of what an account may do.

## Decision

The state:

- A workspace of the state holds `permissions`, the list of the workspace, and `channel_permissions`, one list for each channel that has another one. Both are taken from the snapshot as they are. An entry for a channel that the snapshot does not carry is dropped.
- The permissions of the account in a channel of the channel lists are the entry of the channel, or the list of the workspace when the channel has no entry.
- No event changes a list. An event that changes a role, the roles of the account or an override changes that object in the state, and the list changes with the snapshot that follows `permissions_changed`.
- A channel that leaves the channel lists, by a deletion or by archiving, takes its entry with it.
- A workspace that an event brings holds an empty list until its snapshot. The server records `permissions_changed` for the accounts that see a new workspace (ADR 0310).
- A channel that an event brings has the list of its workspace. A fork and a restore of a channel with overrides record `permissions_changed` (ADR 0310).

An archived channel:

- The permissions of the account in an archived channel are a collection that a reader opens and releases (ADR 0760), read from `GET /api/v1/channels/{channel_id}/permissions`. An answer that names another channel is refused.
- No event carries the list. It is read again after each snapshot, as every open collection is, and a change of it is `permissions_changed`.
- Until the collection is loaded the list is not known, and the client offers nothing that needs a permission in that channel.

What is removed:

- The function that summed the permissions of the roles of the account, and the function that applied the permission function of the server to the loaded overrides. The client compares no role and no override for a permission.
- The client core still depends on `junto-authz`, for role order only (ADR 0791).

The window:

- The member list of a channel, which "Members" in the header of the channel opens, is named "Members of #name" and lists the members of the workspace who can see the channel. The client holds only the permissions of its own account, so it asks the server: `GET /api/v1/channels/{channel_id}/viewers` gives the members of the workspace of the channel for whom the permission function allows "view channel" there, to a caller who sees the channel (`ChannelViewers`). The server checks each member as a request of that member would be checked, so the cost of an answer grows with the members of the workspace. The window reads the list when the member list of a channel is shown, keeps it per channel, and reads it again when it is shown after a change of roles, members, accounts or rules of the server. A change of the name or the picture of an account does not make it old. Until the first answer the list states that it is read; an old list stays shown until the new answer, and stays when that read fails. Reason: the list named the whole workspace, also in a private channel. Tests: `channel_viewers::the_viewers_of_a_channel_are_the_members_who_may_see_it` of the server API tests and `people::the_member_list_of_a_channel_lists_the_people_who_can_see_it` of the window tests.

- A control of one channel reads the list of that channel: the deletion and the pin of the message of another person, the fork of a thread, "Channel settings" in the menu of a channel and the pages of its settings, the restore and the export of a channel, the choices of the overrides page, `@everyone` in the composer, the recording control, the lock, meeting mode, and the menu of a person in a voice channel. A move of a person is offered into a voice channel in which the account holds "move members" too (ADR 0511).
- A control of a workspace reads the list of the workspace: the pages "Channels" and "Roles", the audit log of the workspace, a new channel and a new category, and the restore and the deletion in the list of archived channels.
- A row of the page "Channels" reads the list of its channel: "Move to…" needs "manage channel" in the channel, and a move by one place needs it in every channel whose position the move changes.
- The server permissions of the account are in the account of the snapshot, as before.
- The window holds open the permissions of an archived channel while the main area shows that channel and while its settings are open.

How the window offers an action (`ui::Offer`):

- An action that the person's permissions do not allow is absent, in bars, headers, panels and menus (`Offer::permitted`). No control states a missing permission: no disabled control with a permission sentence, no "Permission: …" line under a menu item, no sentence about a missing permission in a bar or a header.
- An action that the person may take but that the present state does not allow is shown disabled, with its reason, in a menu as the second line of the item (`Offer::now`, `Offer::permitted_now`), such as "Lower hand" while no hand is raised, a move at the end of the list, or "Log out" without a logged-in account.
- Two items are absent, not disabled, where they make no sense: the read or unread item of a place without messages or of a thread without replies, and "Who reacted" on a message without reactions.
- Exceptions, stated with `Offer::same_for_everyone`: the record control of the header of a call shows the same control to every person in the call, disabled without "record calls" and without a sentence; the room settings of a call show their settings to every person in the call, disabled without the permission, because they state the state of the call.
- Reason: a control that a person can never use is noise in every view of that person, and Discord, Slack and Teams hide such actions; a state that will change (a raised hand, an end of a list) is shown with its reason so the person can find the action.
- `Button::offer` and `MenuItem::offer` draw a control as an `Offer` states; the caller draws nothing for `Offer::Absent`. Test: `ui::offer::tests::a_missing_permission_hides_and_a_state_disables_with_its_reason`.

The permissions of using a channel (ADR 0301):

- Writing. Without "send messages" the main area shows a line in place of the composer: "You cannot send messages in this channel." The composer of a thread needs "send messages in threads", and the composer of a thread without a reply needs "create threads" too, because the first reply creates the thread; without them the thread panel shows the same kind of line. These lines state the state of the place, not a control, and stay. A reply and an edit of an own message are offered only where the person may send, because the server takes neither from a person who may not send in the place of the message. The deletion of an own message stays.
- Files. Without "attach files" the paper clip is absent. A file of a drop and a pasted picture are refused with a sentence, and files that were attached before the permission was taken are not sent.
- Reactions. Without "add reactions" the reaction control of a message, its entry in the menu of the message and the control after the reactions are absent. A reaction of another person takes no click; an own reaction can still be removed, as the server allows.
- Threads. "Reply in thread" on a message without a thread needs "create threads" and "send messages in threads"; without them it is absent from the action bar and the menu. Every thread is read from the history, so without "read message history" the thread control of each message shows the count of its replies without an act. The fork bar of a thread is absent without "manage channel"; the channels that the thread was forked into stay listed.
- Mentions. The mention picker offers a role only with "mention roles", and `@everyone` only with "mention everyone".
- History. Without "read message history" the timeline of the channel holds the messages of the current connection and has no more messages before or after (ADR 0721), so the client asks for no earlier page. The list shows the line "Earlier messages are not available: you cannot read the message history of this channel." in place of the line of the start of the channel, and in place of the statement of an empty channel. When the permission returns, the next snapshot reads the history again. The Files and Pins buttons of the header and "Files" of the channel menu are absent; such a channel is no filter of the search; the export of the settings of the channel needs "export channel" and "read message history" and is absent without them.
- Calls outside a call. "Join call", "Ask to enter" and "Join call" of an invitation need "connect" and are absent without it; a locked or full call shows no box to ask to enter.
- Calls during a call. The calls of the media engine read the permissions of the voice channel of the call from the state of the server, as they read the blocks. Without "speak" the microphone and the sound of a share are closed as by a block (ADR 0510), and the microphone control of the profile strip and of the separate call window is drawn held, as for a block, with the tooltip "Your microphone is closed in this call": the profile strip always has its microphone, and the held mark tells why nobody hears the person; the microphone item of the call menu is absent. Without "video" the camera stops and its control and its menu item are absent. Without "share screen" a running share stops and the share control is absent. A start of a source that is not allowed is not made. Without "use voice activity" the engine sends the microphone only while the push-to-talk key is held, whatever mode the person chose; the call strip shows the key, the voice settings show the key and the reason, and the person's own choice is kept. When "speak" returns while the person's own setting is open, the person is told that the microphone is open again. The server also restricts the token of the call to the allowed sources and removes a track of another source; the client does not depend on that removal.
- The moderation items of the menu of a person in a call (move, block microphone, block sound, disconnect) are offered only with their permissions, without a second line.
- Administration. The actions of a page that the permissions refuse are absent: the archive, restore, export and deletion of a channel, the restore of an archived channel, the logo, rename and deletion of a workspace, "New workspace", "New access token", the logo of the server, and adding a member; the sentences that named the missing permission are removed. Only the owner deletes a channel with permanent messages, so the deletion is absent for another person.
- The user limit of the room of a call takes "manage channel" or "set call user limit".
- Each of these controls reads the list that the snapshot after `permissions_changed` states, so a change of a role, of an assignment or of a rule changes the controls without a restart.

## Consequences

- A person who holds a permission by an override of one channel is offered its actions in that channel, and a person whose role is denied a permission in one channel is not.
- A person without "read message history" sees no earlier message of the channel and no read position; the unread sign of such a channel is not read from the server, because the server refuses the summary.
- The client trusts its own state for the push-to-talk key: the media server can restrict the sources of a token, and cannot tell whether a key was held.
- The window follows a change of a role, of an assignment and of an override with one snapshot request, without a rule of its own.
- Between an event that changes a role and the snapshot after `permissions_changed`, the window shows the role as changed and the controls as before.
- The list of archived channels offers restore and deletion by the list of the workspace. An override of one archived channel can differ; the server decides, and the settings of that channel read its own list.
- A state that a test or the screen tool builds by hand states its lists. A helper states them as a server without overrides does.
- The server still decides every request.

Categories:

- The server states no list of the permissions of the account in a category. The page of the rules of a category evaluates them with `ChannelScope::usable_permissions` of `junto-authz`, the function that the server uses: the list of the workspace that the server stated, then the rule of the "everyone" role, the rules of the roles of the account and the rule of the account in the category. An administrator holds every permission. The result decides only which choices the page disables before a request; the server decides each request. Test: `state::admin::view::overrides::tests::the_permissions_of_the_person_in_a_category_follow_its_rules`.
