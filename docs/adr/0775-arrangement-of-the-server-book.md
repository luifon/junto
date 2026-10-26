# 0775. The arrangement of the server book

## Status

Accepted

## Context

The server book listed the servers in the order of its file, with "Move up" and "Move down" in the menu of the selected server (ADR 0780), and the workspaces of each server in the order of the server, always expanded. A person with several servers and several workspaces could not group them, put them in an order of their own, or hide the workspaces that they do not use now.

Discord lets a person organise the list of servers: a server is dragged to another place; a server dropped on another server makes a folder of the two; a server is dragged into a folder, out of it, and within it; a whole folder is dragged; a folder has a name and a colour, is collapsed and expanded with a press, and shows, collapsed, the pictures of its first servers, an unread mark and the sum of the mentions of its servers; its menu has "Mark folder as read" and "Folder settings"; the order, the folders and what is collapsed are kept. The channel list of a Discord server has categories that collapse, with "Collapse all categories". Each drag action of Junto also has a path through a menu that the keyboard reaches (ADR 0702).

## Decision

The arrangement, in the client core (`arrangement`):

- The arrangement belongs to the data directory, in the file of the server book (format version 7; a book of version 6 is read as one without an arrangement). Junto has no account across servers, so nothing synchronises it between devices.
- The order of the servers stays the order of the entries of the book (ADR 0780). A group stands at the place of its first server, and its servers follow in the order of the book; every change writes the entries so that the servers of a group follow each other.
- A group has an identifier, a name of at most 64 characters (it can be empty: the window then names the group by its servers), a colour from a fixed set of the colours of the theme (bone, terracotta, ochre, olive, cypress, bronze, cinnabar) and a collapsed state. Each server can have a record: its group, whether its workspaces are collapsed, and the order of its workspaces (at most 512). A workspace that the order does not name follows those that it names, in the order of the server, so a new workspace appears at the end. Groups are not nested. The book holds at most 128 groups.
- The changes are: move a server next to a server or a group, to the end of a group or to the end of the book; drop a server on another one (a new group of the two at the place of the other one, or its group when it has one); a new group of one server; leave a group (the server stands directly after it); move a group next to a server or a group (a group never goes into a group); rename, colour, collapse and dissolve a group (its servers stay in their place, outside a group); collapse the workspaces of a server; collapse or expand everything; move a workspace among the workspaces of its server. A change is made under the lock of the book on the file as it is then (ADR 0780). A change that names a server, a group or a workspace that is gone changes nothing and fails.
- A group that holds no server any more goes, and a removed server takes its record with it, at every write of the book.
- One step of the keyboard (`arrangement::step`) moves a server among the servers of its group, and a server outside a group or a group among the top-level parts, over a whole group. The step is counted when the write runs, so two steps in a row move two places (ADR 0780).
- Tests: `arrangement::tests` (every change, the steps, the bounds), `book::tests::the_arrangement_is_written_with_the_order_and_a_book_of_version_6_has_none`, `book::tests::an_arrangement_outside_its_bounds_is_a_corrupted_book` and `manager::tests::the_arrangement_of_the_book_orders_the_list_and_is_read_at_the_next_start`.

The server book, in the window:

- Expanded, a group has a head: the mark of its collapse, a square of its colour, its name, the number of its servers, the count of the join requests that wait on its servers, and the sign of what its servers hold unread (the count of the mentions, or a dot). Expanded, its servers follow with a band of its colour at their left. A press on the head collapses or expands the group.
- Collapsed, the server book shows a group as a tile: collapsed, the pictures of its first four servers on the wash of its colour, with the unread sign and the count of the waiting join requests at its corners; expanded, a folder in its colour above the icons of its servers on the wash of its colour. A press collapses or expands it. Collapsed, the head and the tile carry the mark of the selection when they hold the selected server.
- A server with workspaces has a button on its head that collapses its workspaces. Collapsed, the head shows everything that the server holds unread, as no row of a workspace shows it. The header of the server book has one button that collapses every server and group while one of them is expanded, and expands them all otherwise.
- Drags: the head of a server, the head or the tile of a group and the row of a workspace are dragged. Over the head of a server, the upper and the lower third put a dragged server before and after it, and the middle third makes a group; over the head of a group, the upper third puts a dragged server before the group, the middle third at the end of the group, and the lower third at its first place when it is expanded or after it when it is collapsed. A group goes before or after by the upper and the lower half. A workspace goes before or after another workspace of its own server by the halves. The free space after the last server takes a server or a group to the end. The target shows a line at the place, or a frame for the middle third.
- Menus (the keyboard path of each drag): the menu of the selected server has "Move up" and "Move down" (among the servers of its group, or over a whole group), "Group…" (a new group, "Move into" each group, and "Remove from the group"), "Collapse workspaces" or "Expand workspaces", and "Mark server as read". The menu of a group opens with the secondary press or the menu key on its head or tile: "Rename…" (a dialog with the name), "Collapse group" or "Expand group", "Move up", "Move down", "Mark group as read", the colours, and "Dissolve group". The menu of a workspace opens with the secondary press or the menu key on its row: "Move up", "Move down" and "Mark workspace as read". An item at the end that it leads to stays in the menu, does nothing and says why. A change that failed is stated at the end of the menu, or under the header of the server book after a drop.
- Mark as read moves the read position of every place that holds unread messages to its latest message: of one workspace, of a server (also its direct conversations), or of every server of a group.
- The window derives the groups and the top-level parts of the book (`ShellView::groups` and `ShellView::book`) from the list of the client and the arrangement, and the servers keep their indexes in `ShellView::servers`.
- Tests: `state::view::book_tests::arrangement`, `state::book::tests::a_drop_makes_the_change_of_its_zone_and_target`, `state::book::tests::a_drag_has_a_zone_of_a_third_or_of_a_half_of_its_target`, and the window tests `book::servers_are_grouped_by_dragging_and_by_their_menus_and_a_second_start_shows_the_groups` and `book::the_workspaces_of_a_server_are_ordered_collapsed_and_marked_read_and_a_second_start_shows_it`. The screens `book-group-*`, `book-workspaces-collapsed`, `book-workspace-menu` and `book-menu-in-group` of `junto-screens` draw them.

## Consequences

- A person orders and groups the servers and the workspaces with the pointer or with the keyboard, and a second start shows the same server book.
- A client of an earlier version does not read a book of version 7, and does not change it (ADR 0710).
- A new server is added at the end of the book, outside every group, where Discord puts it at the top; ADR 0780 keeps the end.
- The arrangement is per data directory: a second device has its own.
