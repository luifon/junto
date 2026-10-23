# 0780. The order of the server book and the update notice

## Status

Accepted

## Context

The server book lists the servers in the order in which they were added, and a person could not change it. The collapsed server book marks a server that holds unread content with a dot; the expanded server book showed unread content on the rows of the workspaces only, so unread direct messages of a server that is not shown had no sign there.

ADR 0704 decides that the client checks for a new version at start and periodically and shows a notice. The project has no release page and no signing keys yet, so nothing can be downloaded or verified. A server can require a newer client (ADR 0713); the window stated that without naming a version.

## Decision

Order of the server book, in the client core:

- The order of the servers is the order of the entries in the file of the server book. No entry holds a position.
- `ServerBook::move_entry` moves one entry to a position under the lock of the data directory. It reads the file again under the lock, names the entry by its server identifier and its generation, and writes the file in the new order. An entry that the file does not hold, or holds in another generation, is "not saved", and nothing is written. A position past the end is the end.
- `Client::move_server` takes a position in the list of servers that the client shows. The file can hold entries that another program added and that this client does not show: the moved entry is put directly before the entry that follows it in the list of the client, and at the end of the file when none follows. The list of the client then takes the order that the book returned. One lock of the client covers both steps, so two moves leave the list in the order of the later one. The client tells a change after the move.

Order of the server book, in the window:

- The menu of the selected server has "Move up" and "Move down". Each moves the server one place. These actions provide a keyboard path.
- The item at the end that it leads to is disabled and says why in its second line. A disabled item of a menu stays a stop of the keyboard, so the keyboard focus stays on an item that a move disabled. The menu stays open after a move.
- The write runs on the queue of writes of the application (ADR 0730), on a background thread, after the writes that were asked for before. The place is counted when the write runs, so two presses in a row move the server two places. A write that failed is stated in the menu of that server.
- The rows of the server book follow the list of the client core. The selection is a server, so it stays on the moved server.

Unread sign of a server:

- The row of a server has a second unread value: what the server holds unread that no row of a workspace shows. These are the direct conversations, and every place when the server lists no workspace.
- In the expanded server book the head of a server shows that value with the sign of the channel list: the count of the mentions when there is one, and a dot otherwise. The notification settings decide what counts, as for a channel (ADR 0409).
- The collapsed server book keeps its dot, which stands for every unread place of the server.
- A server that shows no lists has no sign. A server in the connection mode "when selected" has no connection while it is not selected (ADR 0107), so its signs do not change until it is selected.

Update check, in the client core (`updates`):

- A version is three numbers, compared from the first one on. It is read from text of three decimal numbers with dots, without a sign, without a leading zero and without a suffix. The version of a build is the version of its package.
- A source of releases is an interface with one operation: the newest release. A release has a version and an optional page. `ClientOptions` takes the source and the version of the build. Without a source no request is made and the state stays "not checked".
- The states are: not checked, current, available with the release, and failed. A release that is not newer than the build is "current". A check that fails after one that succeeded keeps the answer of that one; "failed" means that no check has succeeded.
- The client checks once when its event streams start and every 6 hours after the end of the check before. One request runs at a time: a check that is asked for while one runs takes its answer. A source that has not answered after 60 seconds has failed. A check runs to its end when its caller is gone, and the periodic check ends with the client.
- The page of a release is kept only when it is an `https` address of at most 512 bytes with a host, without a user name or a password, and without a control character or a space. It is held in the form that a URL parser writes. Every other page is left out.
- A change of the state is a change of the client, so a reader of the client draws again.
- No real source exists in this version. The application passes no source. Tests pass a source that answers with a fixed release and counts its checks.

Update notice, in the window:

- While the state is "available", the channel column shows one line directly above the profile strip: the download icon, the sentence "New version available", and the action at the right end. The server book shows nothing of the notice, expanded or collapsed.
- The window draws the channel column with its profile strip in every state: for a server that shows its lists, for a server that shows a state panel in the main area, and while the server book holds no server. The line is drawn in all of them. It has no second place.
- The form uses logical pixels at 100 %: a height of 30, a margin of 8 at the sides and below and none above, a padding of 9 at the sides, a gap of 7, a corner radius of 3, the cypress wash as the ground with its border of 1 pixel, and text of 13 pixels with the weight 500 in cypress. The action is bold and underlined. The sentence is cut with an ellipsis before it can move the action.
- The action is "Details" and opens the version settings; the update format and installation are described in ADR 0724 and ADR 0726. The sentence names no version; the accessible name and the tooltip of the line are "Junto <version> is available.".
- The whole line is one control with the role of a button. It is a stop of the keyboard, and a click, Enter or Space activates it. It opens the settings at their last section, "Version". The content of a dialog scrolls in a window that is lower than the dialog; the settings give the dialog the position of their content, so that this section is shown.
- No control puts the line away, and the window holds no state of the notice beside the state of the check. The line leaves when a check finds no release that is newer than the build.
- The section "Version" of the settings states the version of this build, the state of the check, "Check now" when a source exists, and that this version does not install updates by itself. For an available release with a page it shows the address in a field that takes no change, so that it can be selected, and "Copy address". The client opens no browser.
- The state panel of a server that needs a newer client names the available version when one is known.
- The version is drawn from its three numbers. The address is the only text of the source that the window shows: it has a field of its own of one line, so it cannot move a control, and it is in no sentence.
- Nothing is downloaded and nothing is installed.

## Consequences

- A person orders the servers with the pointer or the keyboard, and a second start shows the same order, because the order is the file.
- A second program that changes the server book between two moves does not lose its entries: every move reads the file again under the lock.
- Unread direct messages of a server in the background are visible in the expanded server book without selecting that server.
- The application makes no request outside its server book in this version, because it has no source. ADR 0704 stays the decision for the real source: its request, the signing keys, the download and the install are not built.
- With a source, the client makes at most one request every 6 hours by itself, and one per press of "Check now" while no check runs.
- A person sees the notice in every state of the window while a newer release is known, also in a narrow window, where the server book is collapsed. A person cannot put it away. The line takes 38 logical pixels of the height of the channel list.
- A person copies the address of a release and opens it in a browser by hand.
