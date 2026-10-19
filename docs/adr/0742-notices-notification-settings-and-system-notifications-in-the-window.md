# 0742. Notices, notification settings and system notifications in the window

## Status

Accepted

## Context

ADR 0404 defines unread signs, the notification level, the mute and "do not disturb". ADR 0409 defines their routes: one notice per recipient and message without text and without a place, a level for the account on a server, for a workspace and for a channel, a mute and two mention switches for a channel, and "do not disturb" with a daily quiet time as data of the account on one server. ADR 0722 gives the window the notices of the state, the messages that they name, the notification settings and the answer whether the account is in its quiet time. The approved design draws no list of notices and no notification settings. gpui shows a notification of the operating system, takes one away by its tag, and tells a press on one; on macOS that works only from an application bundle, and the system asks the person for the permission at the first notification (ADR 0701).

The server marks every notice up to a time as read with one request, and states with the snapshot whether a workspace has a level of its administrators (ADR 0412, ADR 0792). It has no setting for a thread. The server tells no notice while the account is in a quiet time, so a client learns of the notices of that time when the quiet time has ended.

## Decision

List of notices:

- The channel column has an entry "Notifications" above the channels, with the count of the notices that the person has not read. It opens the notices of the shown server in the side panel. The panel shows one list: opening the notices closes a thread, the pinned messages and a transcript, and takes a search out of the panel.
- A row shows the place of the message, its day, its author, its time, what the notice is (a mention, a direct message, a reply in a thread, a new message) and the start of its text. A notice whose message the server no longer has is not listed, and neither is one whose place the state does not hold. A notice whose message could not be read is listed without its text and leads nowhere.
- A channel that is in no channel list can be archived: the state knows an archived channel from the archived list of its workspace (ADR 0762). While the list is open, the window holds the archived list of each workspace that a notice names with such a channel, and a notice of an archived channel is listed with its place and leads to it. The list stops at the first notice whose archived list has not answered and is marked as reading: the later notices follow at the end when the list answers, so no listed row moves (ADR 0730).
- A press on a row shows the message in its place and marks the notice as read. The channel or the conversation is selected, the thread of a reply is opened over the list, and the messages around the message are read when the list of the place does not hold it. The list goes to the message and marks it, and does not follow new messages then. The mark stays until the person selects another place. A second press on the same row goes to the message again.
- "Mark all as read" sends one request that marks every notice up to the newest unread notice that the state holds; a notice that arrives later stays unread. The panel shows the marking until the state holds the event of the change (ADR 0763), and a press meanwhile sends nothing. A refusal is stated in the panel.
- The panel offers "Mark all as read" while the state holds an unread notice whose message is not gone, also while the messages of the notices are read. After a reload of the state the messages that the notices name are kept for display and read again (ADR 0721), so the rows stay.
- The read mark of the notice of a row is sent by itself. A mark that is asked for while marks of the server are on their way follows them in a request of its own; a mark that waits or is on its way is not sent a second time. An answer "not found" is no failure: the notice is gone. A read mark of a notice does not move a read position.
- "Show older" reads the page after the listed notices.
- The view of the call of this device has its own side panel: the entry of the notices takes no click there and tells the reason, as the thread rows do (ADR 0741).

Notification settings:

- One dialog shows the settings of one scope: the server (the account on that server), a workspace, or a channel. The menu of the server opens the first two, and the menu of a channel the third.
- The level is one of "all messages", "mentions only" and "nothing". A workspace and a channel also have "default", which takes the level of the scope above (ADR 0409). The dialog states which level "default" gives now. The state holds the level of the administrators of each workspace, or that the workspace sets none (ADR 0412, ADR 0792); with none, the level of the account holds.
- The dialog of a channel also has the mute with its four lengths, and the two switches for mentions of a role and of everyone.
- The dialog of a workspace shows the level of its administrators as the state holds it. An administrator of the server sets it there; another member reads it. The dialog and the page "Notifications" share one change of that level (ADR 0763).
- The workspace dialog reserves a scrolling status area for a pending default request and the explanation that no default is set, as well as its refusal area. Starting and completing a request keeps the dialog height and the bounds of its controls unchanged (ADR 0730).
- A change is one request that replaces the settings of its scope. One change of a scope is on its way at a time. The dialog shows what the server took until the state of the server holds the events of the change (ADR 0763). A refusal is told in the dialog of the scope, and above the composer when the change came from the menu of a row.
- A channel row opens a menu with a right click, the Menu key, or Shift and F10: the notification settings, and the mute for one hour, for eight hours, until the next day begins in the time zone of the person, or until it is turned off. A muted channel states the end of its mute there and offers "Unmute". The row of a direct conversation has the same menu for its mute (ADR 0515).
- A place is drawn as muted when its summary says so, and when the stored settings say so and their end has not passed: a channel without a message has no summary.

Quiet times:

- "Do not disturb" and the daily quiet time are in the settings of the application. A change is sent to every server on which the device has a session, because ADR 0404 defines them for the whole application and the server stores them per account. A server that does not take the change is named with its reason, and so is a server whose state the device does not hold at that moment, or whose settings of the account are being changed.
- The switch shows "on" when every such server says so. When the servers differ, the settings say so, and the next change sets all of them.
- The daily quiet time is typed as two times of day in a named time zone. The settings show the zone of the stored quiet time, and otherwise the zone of the system; "Change time zone" opens a search of the names of ADR 0798, and a choice is sent with the next change. The settings state the zone of the system when the chosen zone is another one, and state when the build does not know the stored zone. The client sends the times as minutes of a day with the name of the zone (ADR 0412), and applies the quiet time to what it holds by the clock of that zone. Two equal times are refused before a request.

System notifications:

- The state of the application decides what is shown with one function without side effects, and tells it to an interface with two operations: show, and take away by a tag. The binary gives the interface of gpui. Every other start of the application state, such as a test and the tool that draws the screens, gets one that keeps what it was told. No automated run shows a notification or makes the system ask for a permission.
- A notice gets a system notification when all of this holds:
  1. it appeared after the first state of its server in this run of the application, is unread, and is newer than every notice that was seen before;
  2. the account of that server is not in its quiet time now, the state in which the notice appeared is not the one in which "do not disturb" ended, and the notice was not made inside the daily quiet time;
  3. the person did not mute its place;
  4. the window is not the active window of the system, or does not show the place of the message: the channel or conversation in the main area, and for a reply of a thread also its thread in the side panel.
- The level is not evaluated by the client: a notice exists only when the level of its place wants it (ADR 0409).
- A notice waits for the read of its message, at most 32 at one time, and is shown with the author, the place and the start of the text. A message whose read failed is shown without text.
- The title and the text of a system notification are one line each, without control characters, of at most 120 and 160 characters.
- One server shows at most 3 system notifications within 10 seconds. A notice or an invitation above that bound gets none later, and neither does a notice that waits for the read of its message when the bound is reached: the notices are in the list, and the window draws the invitation.
- An invitation into a call gets a system notification unless the account is in its quiet time or the window is the active window, which draws the invitation (ADR 0739). It comes before the notices of the same change of the state. An invitation that the first state of a server holds follows the same rule: it waits for an answer at that moment. A call in progress changes no rule: a message in the chat of the call of this device follows rules 1 to 4.
- A system notification is taken away when its notice was read or is gone, when its message is gone or deleted, when the state no longer holds the place of its message, when its invitation ended, when the session of its server ended or the server was removed, and when the application forgets it: it remembers 64 shown notices of one server. The notifications that are shown when the application ends stay in the system.
- A press on a system notification brings the application to the front, marks the notice as read and shows its message as a row of the list does. A notice whose message could not be read shows its server, and so does an invitation. What a press leads to is kept as long as the notification is shown.
- With more than one saved server the title of the system notification of a notice names its server, and the text of the one of an invitation does.

## Consequences

- The notices and the notification settings have no screen in the approved design. They are built from its components.
- "Mark all as read" costs one request.
- A thread has no notification setting, because the server has none.
- The settings show the time zone of the daily quiet time, and the person chooses another one from the list of ADR 0798.
- A device with sessions on two servers can hold two values of "do not disturb" after a change on another device. The settings say so.
- The first notice after "do not disturb" ended can miss its system notification when it arrives in the same change of the state as the notices of the quiet time.
- On macOS the system notifications need an application bundle, which the build does not make yet (ADR 0701). `cargo run` shows none.
- The window tests against the real server cover a mention that makes a notice and a system notification, the rules for an active window and for a quiet time, the jump from a notice, "mark all as read", the level and the mute of a channel from its menu, the dialogs of the server and of the workspace, and the loss of the notices of a channel with the access to it.
- New direct dependency of the desktop client: `uuid`, for the identifiers of notices and of the scopes of settings. It was in the lock file already.
