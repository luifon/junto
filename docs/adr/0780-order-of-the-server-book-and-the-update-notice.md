# 0780. The order of the server book and the update notice

## Status

Accepted

## Context

The server book lists the servers in the order in which they were added, and a person could not change it. The collapsed server book marks a server that holds unread content with a dot; the expanded server book showed unread content on the rows of the workspaces only, so unread direct messages of a server that is not shown had no sign there.

ADR 0704 decides that the client checks for a new version at start and periodically and shows a notice. The project has no release page and no signing keys yet, so nothing can be downloaded or verified. A server can require a newer client (ADR 0713); the window stated that without naming a version.

A server states its release version in the `hello` of each event stream connection, but the window did not show it, so a person could not see which version a server runs or whether a newer release of the server exists. The release information of the client (ADR 0724) names no server version.

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
- The check also keeps the version of the newest release that the last trusted answer named, also when that release is not newer than the build (`UpdateCheck::newest`). An answer that has expired or that no trusted key signed leaves no version; a check that fails in another way keeps the version of the check before it. Without a source there is no version.
- A change of the state or of the newest version is a change of the client, so a reader of the client draws again.
- The application passes the compiled source of ADR 0724 (`HttpSource`). Tests pass a source that answers with a fixed release and counts its checks.

Update notice, in the window:

- While the state is "available", the channel column shows one line directly above the profile strip: the download icon, the sentence "New version available", and the action at the right end. The server book shows nothing of the notice, expanded or collapsed.
- The window draws the channel column with its profile strip in every state: for a server that shows its lists, for a server that shows a state panel in the main area, and while the server book holds no server. The line is drawn in all of them. It has no second place.
- The form uses logical pixels at 100 %: a height of 30, a margin of 8 at the sides and below and none above, a padding of 9 at the sides, a gap of 7, a corner radius of 3, the cypress wash as the ground with its border of 1 pixel, and text of 13 pixels with the weight 500 in cypress. The action is bold and underlined. The sentence is cut with an ellipsis before it can move the action.
- The action is "Update" where this copy installs updates itself, and it installs the update and opens the version settings; after an installed update it is "Restart", which restarts Junto; elsewhere it is "Details" and opens the version settings. The update format and installation are described in ADR 0724 and ADR 0726. The sentence names no version; the accessible name and the tooltip of the line are "Junto <version> is available.".
- The whole line is one control with the role of a button. It is a stop of the keyboard, and a click, Enter or Space activates it. It opens the settings at their last section, "Version". The content of a dialog scrolls in a window that is lower than the dialog; the settings give the dialog the position of their content, so that this section is shown.
- No control puts the line away, and the window holds no state of the notice beside the state of the check. The line leaves when a check finds no release that is newer than the build.
- The section "Version" of the settings states the version of this build, the state of the check, "Check now" when a source exists, and how this copy gets a newer version (below). For an available release with a page it shows the address in a field that takes no change, so that it can be selected, and "Copy address". The client opens no browser.
- The state panel of a server that needs a newer client names the available version when one is known.
- The version is drawn from its three numbers. The address is the only text of the source that the window shows: it has a field of its own of one line, so it cannot move a control, and it is in no sentence.
- Nothing is downloaded and nothing is installed before the person chooses "Update".

The version of this copy, below "Add server":

- The expanded server book ends with the version line below the field that adds a server, above a rule: "Junto <version>" (with "(test configuration)" for a test build), one sentence of where its update stands, and the button of the next step. The collapsed server book shows the version number below the button that adds a server, and above it the button alone while a newer version is available or installed. The line is one status for the accessibility tree, with the version and the sentence as its name.
- The sentence and the button follow the update check and the way this copy updates (`state::update_install::version_line`): "Up to date."; "Looking for a newer version…" while the first check runs; "Not checked for a newer version yet."; "Could not check for a newer version." with "Details" after a failed, expired or untrusted check; "This build does not look for newer versions." for a build without a source; and for an available version, by the way of this copy:
  - The macOS bundle, the Windows folder, the Linux directory and the AppImage, in a location that the account can write: "Junto <version> is available." with "Update". The update runs the installer of ADR 0726 in the background ("Downloading and verifying the update…"); then "Junto <version> is installed. Restart Junto to use it." with "Restart". A failed update states why and offers "Update" again.
  - A copy in a location that the account cannot write, or that macOS runs from a translocated copy: the step to move it, with "Details".
  - The Debian package (ADR 0728): the name of the `.deb` of the version, with "Details". The section "Version" states that the package manager of the system updates it, the command `sudo apt install ./junto_<version>_amd64.deb` with "Copy command", and the address of the download page with "Copy address".
  - A copy that no package installed: that it does not update itself, the address of the download page, with "Details".
- The download page is the release site whose directory `updates/` is the update source (ADR 0727); the page of a release is the notes page of its version on that site.
- "Restart" quits the application as Quit does (the call is left first) and starts the installed version with the same data directory: gpui starts the same path again after the process ended, the path that the update replaced; for an AppImage the file in `APPIMAGE`, not the program inside its mount, which ends with the process.

Version of a server, in the client core:

- The stream client keeps the release version that the server states in the `hello` of a connection (ADR 0718) and gives it to the readers in the view of the server (`LiveView::server_version`). The `hello` is the first message of every connection, so no request is added. The route `GET /api/server` states no version.
- The version is read as a semantic version of at most 64 bytes, with its pre-release and its build metadata, by the crate `semver` (`ServerVersion`). Other text gives no version, so the window shows only text of a known form.
- The version of the last `hello` stays while a connection is lost and is replaced by the `hello` of the next connection. It is forgotten with the state of the server: at the level `off` and for another account.
- One release has one workspace version for the server images and for the client packages (`docs/release.md`). The release information names no server version, so the newest release that the source of the client names is the newest release of the server. The client reads the release information of the target of its own build.
- A newer release of a server is that newest version when it comes after the version of the server in the order of semantic versions: a pre-release such as `0.1.0-test.2` comes before `0.1.0`, and build metadata has no place in the order. Without a source there is no newest version, and only the version of the server is known.

Version of a server, in the window:

- The header of the menu of the selected server states "Server version <version>" under the name and the address of the server, in the small type and the secondary ink. While the server has stated no version, the line is not drawn.
- When a newer release is known, a second line states "Version <version> is available" in cypress, with the download icon.
- The lines only state facts. They are not items of the menu: the keyboard does not stop on them, and a click does nothing. The client installs nothing on a server; an operator upgrades it with `docs/install.md`.
- Tests of the version line: `state::update_install::tests::the_version_line_states_the_version_and_the_next_step_of_each_way` and `an_appimage_restarts_from_its_file_and_another_package_from_its_program`; the window tests `updates::the_server_book_states_the_version_of_this_copy_below_add_server`, `updates::an_update_from_the_server_book_installs_and_offers_the_restart` and `updates::a_copy_that_does_not_update_itself_states_its_steps_and_the_download_page`; `releases::a_source_in_the_updates_directory_names_the_site_and_the_notes_of_a_version` in `junto-client-core`. The package lifecycle of ADR 0725 updates the real installed client through its window on Linux, macOS and Windows.
- Tests: `version::tests::a_server_states_a_semantic_version_and_a_release_after_it_is_newer`, `stream::tests::the_view_states_the_version_of_the_last_hello_until_the_state_is_forgotten` and `updates::tests::the_newest_version_follows_each_trusted_answer_and_is_told` in `junto-client-core`; `state::book::tests::a_newer_release_of_a_server_is_the_newest_release_after_its_version`, `state::view::book_tests::the_selected_server_states_its_version_and_a_newer_release_of_the_source` and the window test `servers::the_server_menu_states_the_version_of_the_server_and_a_newer_release_of_the_source` in `junto-client`. The screens `server-menu` and `server-menu-newer-release` of `junto-screens` draw both lines.

## Consequences

- A person orders the servers with the pointer or the keyboard, and a second start shows the same order, because the order is the file.
- A second program that changes the server book between two moves does not lose its entries: every move reads the file again under the lock.
- Unread direct messages of a server in the background are visible in the expanded server book without selecting that server.
- With a compiled source the application makes the update check of ADR 0704 as its only request outside its server book.
- With a source, the client makes at most one request every 6 hours by itself, and one per press of "Check now" while no check runs.
- A person sees the notice in every state of the window while a newer release is known, also in a narrow window, where the server book is collapsed. A person cannot put it away. The line takes 38 logical pixels of the height of the channel list.
- A person copies the address of a release or of the download page and opens it in a browser by hand.
- A person sees the version of the running copy in the server book in every state of the window, and updates and restarts from there, from the notice of the channel column, or from the settings.
- The menu of a server states the version that the server runs and a newer release of the server without a request beyond those of the event stream and of the update check.
- The comparison knows only the newest release that the source of this build names. When the client packages of a release are published before its server images, the menu names that release before an operator can install it on the server.
- A server that states no semantic version has no version line.
