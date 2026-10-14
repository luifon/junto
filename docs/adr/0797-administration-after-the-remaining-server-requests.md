# 0797. The administration in the window after the remaining server requests

## Status

Accepted

## Context

ADR 0761 describes the administration layer and ADR 0791 the pages of the roles. The server now takes a reason at a ban and at a removal, removes a member from one workspace, filters the audit log, names the creator of an invite, changes one permission of an override, and states the last device with a code of its own (ADR 0795). The route of the notification level of a workspace answers that no level is set and removes a level (ADR 0412); no page of the window set that level. The approved design has no view for any of these.

## Decision

Reason:

- The question before a ban, before a removal from the server and before a removal from a workspace has one field for a reason under it. The field is optional. A text of more than 1,000 characters or with a control character holds the confirmation, and the page says why. The reason is sent without the space around it.
- The page states under the field that the reason is kept in the audit entry and is not sent to the person.
- A row of the audit log shows the reason of its entry as a line of its own under the sentence of the action, without a control character and cut at 1,000 characters. The sentence of the action does not hold the reason.

Removal from a workspace:

- A row of "Members" has "Remove from workspace" for an administrator. Every other person does not have the action: the server gives the removal to administrators with a session only (ADR 0106).
- The row states before a request why the action is not possible: the member is the server owner, the member is the own account, the member stands at or above the person in the workspace (ADR 0309), or the accounts and the order of the roles are on their way. The sentence of role order stands in the row. A read that failed enables the action, and the server decides.
- The question names the member and the workspace and states what the member keeps. A question whose removal is no longer possible is put away; while the order is read again it stays and its confirmation waits.
- "Members" reads the accounts and, for an administrator who is not the owner, the server roles.

Audit log:

- The filters are applied by the server: one action, the person who acted, the person concerned (an identifier that the entry names), and a period of 24 hours, 7 days or 30 days before the read. A change of a filter empties the page and reads the log from its start; "Load older" repeats the filters and the start of the period of the first page; "Read again" reads the first page with the period counted from that moment. The filter on loaded entries by a group of actions is removed.
- The filter by action lists the actions of one group at a time, by the name of the protocol with spaces in place of underscores. The list of actions is a table of the window; a test compares it with the actions of the protocol.
- The two filters by person offer the accounts and the members that the state holds, the accounts that acted in a page that was read for the shown log, and the accounts of the filters. The page reads the accounts for a person who may list them.
- A filter belongs to the shown log. Another log starts without a filter.

Invites:

- The row of an invite names its creator by the name that the state holds, and otherwise by the name that the server states with the invite. The name of the server is shown without control characters and cut at 64 characters.

Overrides:

- A choice of one permission sends one request that names that permission and its new state. "Private channel" sends the state of "view channel" of the "everyone" role. The removal of an override stays one request with two empty sets.

Notification level of a workspace:

- The page "Notifications" of the shown workspace, for an administrator, has one control with four choices: "Not set", "All messages", "Mentions only", "Nothing". "Not set" removes the level. The page states under the control what holds for a member with the present choice.
- The control shows the choice of the state of the server and takes no choice while a request of the level is on its way. Every other choice is sent, also the one that the control shows: the answer of a request can arrive before the event of its change, the control shows the level of before for that time, and a choice of that level reverses the change. The server changes and records nothing for the level that it holds already. The window does not wait for the event: the answer and the events have no common order, so no comparison of levels tells that the event of the own change has arrived.

Last device:

- The error code `last_device` has the sentence of the last device at every control. The device list also states the rule before a request, as before, and keeps the sentence for a server that answers `conflict`.

Pictures:

- A picture that an account set has a readable alternative in the lists of people of the administration and in the member list: an image in the accessibility tree with the label "Picture of" and the name (ADR 0702). The member list of the server states which members set a picture, so the alternative is present before the image is read. A generated picture has none.

## Consequences

- A filter of the audit log finds entries that the page had not loaded. An entry that passes no filter is never sent to the client.
- A period is counted from the time at which the first page is read: a change of a filter, "Read again" and a read again after a failure count it anew. "Load older" repeats the start of the first page, so both pages belong to one list. A page that stays open without a new read does not move its period.
- A change of one permission of an override does not undo a change of another permission that another person made in the same moment, and the grant rule is applied to the changed permission only.
- An administrator sees why a member cannot be removed from a workspace before a request is sent. The rule of the page is the function of the server; when they differ after a change, the refusal of the server is stated at the row.
- The page "Notifications" exists for administrators only. A member reads the level of a workspace in no page of this window.
- Sentences of ADR 0761 that this record replaces: a removal and a ban carry no reason; the filters of the audit log work on the loaded entries.
