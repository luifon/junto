# junto-client

The Junto desktop application: a gpui window on `junto-client-core` and `junto-media`. The binary is `junto`. The design decisions are in [`docs/adr/`](../../docs/adr/README.md): ADR 0730 describes this crate, ADR 0731 its theme and components, ADR 0732 joining a server in the window, ADR 0733 the media engine, ADR 0734 the call of a client, ADR 0735 the screens of a voice call, ADR 0736 the screen capture, ADR 0737 shares in the media engine, ADR 0738 the screens of a share and of a recording, ADR 0739 the rules of a call, ADR 0740 the message list and the composer, ADR 0741 the thread panel, pins, permanent messages and direct conversations, ADR 0742 the notices, the notification settings and the system notifications, ADR 0743 the search, ADR 0744 the card of a recording and the transcript, ADRs 0760–0762 administration and archived channels, ADRs 0770–0773 cameras, direct calls, detached windows and screen shares, ADR 0780 the order of the server book and the update notice, ADR 0781 the account layer, ADR 0782 device linking, ADR 0783 the pictures of people, ADR 0784 the list of recordings, ADR 0785 the order of call state, ADR 0786 the condition waits of macOS, ADR 0790 the permissions that the server states, ADR 0791 the pages of the roles, ADR 0796 the followed threads and the follow control, ADR 0797 the administration after the last server requests, ADR 0798 the time zones, and ADR 0799 the logo of a server.

This version shows the servers of the server book, their workspaces and channels, the people in voice channels, and the state of each server. It joins a server with a code or a request for access, and it holds a voice call in a voice channel: microphone, sound, mute, a volume per person, levels, push-to-talk and the choice of the sound devices. In a call a person shares a screen or a window with its sound, watches the shares of the others, starts and stops a recording, and uses the rules of the room: meeting mode with raised hands, the lock, the user limit, requests to enter, invitations, the menu of a person with blocks, removal and move. A person reads and writes messages in a text channel, in the chat of a voice channel and in a direct conversation: formatted text, replies, mentions, reactions, files, edits and deletions, with unread signs in the channel list. A person answers in the thread of a message, forks a thread into a channel, and pins messages. The application lists the notices of a person, takes the notification level of a server, a workspace and a channel, the mute of a place and the quiet times, and shows notifications of the system. It searches messages and transcripts. A finished recording is a card in the chat of its voice channel, with its files and its transcript. It supports cameras, direct-message calls and a detached call window. A person with the permissions manages workspaces, categories, channels, the permissions of a channel, members, bans, invites, join requests, access tokens and exports, and reads the audit log with filters that the server applies. An administrator removes a member from one workspace, sets the notification level of a workspace, and sets the logo of the server, which every window shows in the server book. A person follows a thread or stops following it, and finds the followed threads under their channel. A person orders the servers of the server book, changes the name and the picture of the account on a server, lists, renames and removes the devices of the account, links a second device on one server or on all servers, and sees the picture of each person who set one. A page lists the recordings of a workspace. It has no correction of a transcript. It is built and tested on macOS and in Linux containers. The Linux capture implementation and its verification limits are described below and in ADR 0751; Windows is not supported yet.

A media connection shows "Entering call…" until the server lists this place identity. Controls for a person inside the call become available after that confirmation; callbacks also check the current call identity and admission.

## Run it locally

A server for development, with a temporary database in Docker:

```sh
tools/dev-server.sh
```

The script prints the address (`http://127.0.0.1:8080`) and the server prints its one-time owner code below it. Control-C ends the server and removes its database. `--port`, `--name` and `--media` (a LiveKit server for calls) are its options.

The application, with a data directory of its own:

```sh
cargo run -p junto-client -- --data-dir /tmp/junto-first
```

At the first start the application asks for a name. Type the address into "Add server" of the server book, then the owner code into "I have a code". The first device that presents the owner code is the server owner.

## Two people on one machine

Each instance needs its own data directory: the directory holds the server book, the profile and the device keys of one installation (ADR 0710).

```sh
cargo run -p junto-client -- --data-dir /tmp/junto-first
cargo run -p junto-client -- --data-dir /tmp/junto-second
```

The second person needs an invite code or an approval. The command-line tool of the client core works on the data directory of the first person, also while the application runs:

```sh
# An invite code for the second person, created by the owner:
JUNTO_DATA_DIR=/tmp/junto-first cargo run -p junto-client-core --bin junto-cli -- invite

# Or: the second person presses "Request access", and the owner approves the request:
JUNTO_DATA_DIR=/tmp/junto-first cargo run -p junto-client-core --bin junto-cli -- requests
JUNTO_DATA_DIR=/tmp/junto-first cargo run -p junto-client-core --bin junto-cli -- approve-request <id>
```

With more than one saved server, `junto-cli` names the server with `--server <identifier, address or name>`; `junto-cli` without arguments prints its commands.

## Several servers, the account and its devices

Start a second development server on another port and with another name (`tools/dev-server.sh --port 8081 --name Harbor`), and add it in the first instance with its address.

- **Order.** The menu of the selected server (the button beside the server name) has "Move up" and "Move down". The order is stored in the server book.
- **A server in the background.** While the first server is shown, a direct message or a mention on the second one puts the unread sign on that server in the server book. With "Keep connected" off in the menu of a server, that server connects only while it is shown, and its signs do not change in the background.
- **Your account.** The same menu has "Your account". "Profile" sets the name and the picture of the account on that server (PNG, JPEG or WebP, at most 256 KiB and 1024 by 1024 pixels). "Devices" lists the devices of the account with "Rename" and "Remove", and has "Link a new device". "Recoveries" lists every recovery of the account. "Log out" in the same menu ends the session of this device on that server and keeps the account and its key; the server stays logged out, also after a restart, until "Log in".
- **Recover an account.** A person who lost every device adds the server again and chooses "I already have an account: link this device"; the server owner types the key that it shows under "Recover" in the row of the account on the page "People" of "Administration". The new device logs in by itself.
- **A new direct message.** "New direct message" under the direct conversations lists the people that the person may write to on that server, by a part of their name: the members of the workspaces of the person and the administrators of the server, and for an administrator every account. An account that is a member of no workspace keeps the direct conversations, the notices and "New direct message", and can write to the administrators.
- **Link a second device on one server.** In the second instance, with a data directory of its own, add the server and choose "I already have an account: link this device" in the dialog "Join a server". It shows a link code and the key of the device. In the first instance open "Your account", "Devices", "Link a new device", type the code, compare the key with the one of the second instance, and press "Approve". The second instance logs in by itself within 5 seconds.
- **Link a device on all servers.** The settings (the gear beside the name) have "Devices". The first instance chooses "Link a new device to all my servers" and copies the offer text; the second chooses "Link this device to my servers", pastes the offer, and copies the request text back; the first pastes it, compares the fingerprint of both instances, and presses "Approve".
- **Pictures.** The settings hold the picture of the profile preset, which a new account gets. A picture that a person set on a server is shown wherever the window shows that person.
- **Version.** The settings state the version of the build and whether a release source was configured at build time. A configured macOS bundle offers Update for a newer authenticated release, keeps the previous application for rollback, and asks you to quit and reopen after installation. A build without a source makes no update request.
- **Recordings.** "Administration" has the page "Recordings" with the recordings of the shown workspace that the account may see. The development server has no recorder, so the list is empty there.

## Messages between two instances

Start two instances as above and select the same text channel in both. The development server stores the files of messages in a temporary directory.

- **Write.** Enter sends, and Shift with Enter adds a line. The text takes marks: `**bold**`, `*italic*`, `~~struck~~`, `` `code` ``, three backticks around a code block, `> ` for a quote, `- ` and `1. ` for lists, `- [ ] ` for a check box, and `[text](address)` for a link. The toolbar puts the marks around the selection.
- **Mention.** Type `@` and the start of a name; the arrow keys choose, and Enter or Tab takes the entry. The other instance shows the message with a tinted ground and the sign "@ 1" on its channel when it shows another channel.
- **Reply, edit, delete, react.** The bar at a message under the pointer has the four actions; a right click on a message, or Shift and F10 while it has the focus, opens the same actions as a menu. The arrow up in an empty composer edits the last own message.
- **Files.** The paper clip opens the file dialog; a file can also be dropped on the composer, and a picture can be pasted. The upload starts at once and continues after a cut connection. A picture up to 1,024 pixels per side is shown in the list; every other file is a row. The download control saves a file to the download directory under a name that no file has.
- **Unread.** While one instance shows another channel, a message of the other instance makes the name of the channel bold with a dot. Selecting the channel shows the line "New" before the first unread message. "Mark as read" in the header appears while the channel has unread messages.
- **Without a connection.** Stop the server and send a message: it stays in the list as "Not sent" with "Send again" and "Discard".

The same routes from a terminal, on the data directory of an instance:

```sh
JUNTO_DATA_DIR=/tmp/junto-first cargo run -p junto-client-core --bin junto-cli -- state
JUNTO_DATA_DIR=/tmp/junto-first cargo run -p junto-client-core --bin junto-cli -- \
    messages send <channel id> --text "Hello from the terminal" --file notes.txt
JUNTO_DATA_DIR=/tmp/junto-first cargo run -p junto-client-core --bin junto-cli -- \
    messages list <channel id>
```

`state` prints the identifiers of the channels and the unread counts. `junto-cli` without arguments lists the other actions of `messages`.

## Threads, pins and direct conversations between two instances

Start two instances as above and select the same text channel in both.

- **Thread.** The bar at a message under the pointer has "Reply in thread", and so has the menu of the message. The thread opens in the side panel with the message, its replies and a composer of its own. The message in the channel then shows the count of the replies and the time of the last one. A reply is not shown in the list of the channel.
- **Follow a thread.** The panel states under the head of the thread whether your account follows it: by your choice, because you wrote in it or were named in it, or not. "Follow" and "Stop following" store a choice; "Use the default" removes it. The replies of a thread that you do not follow are not counted as unread; a mention still is. The channel that is shown lists the threads that you follow under its name: the ones with something unread first, then up to 8 others, and a row that shows the further ones.
- **What is new in a thread.** Close the panel in the first instance and answer in the thread with the second one. The first instance shows the count of the new replies on the message, and lists the thread under its channel in the channel column. With the default notification level, the name of the channel gets a sign only for a mention.
- **Fork.** The panel has "Fork thread into…" with "Text channel" and "Voice channel" for a person who holds "manage channel" in the channel; the owner of a new server may do it, a member sees the two controls disabled. The dialog takes a name. The new channel holds a copy of the message and of its replies, and the panel then names the channel in place of the two controls. A second fork of the same thread is refused, and the dialog says so.
- **Pins.** "Pin" is in the bar and in the menu of a message for a person who holds "manage messages" in the channel. "Pins" in the header opens the list of the channel, with "Go to message" and "Unpin".
- **Permanent messages.** `junto-cli` has no command for the setting of a channel; a channel that has it shows "Permanent messages" in its header, and its messages offer no edit.
- **Direct conversation.** Open "Members" and press the button beside a member. The conversation takes the place of the channel, and the channel column lists it under "Direct messages". Two accounts without a common workspace cannot write to each other, unless one of them administers the server; the refusal is stated at the member.

The same routes from a terminal:

```sh
JUNTO_DATA_DIR=/tmp/junto-first cargo run -p junto-client-core --bin junto-cli -- threads list <message id>
JUNTO_DATA_DIR=/tmp/junto-first cargo run -p junto-client-core --bin junto-cli -- threads unread
JUNTO_DATA_DIR=/tmp/junto-first cargo run -p junto-client-core --bin junto-cli -- pins list <channel id>
JUNTO_DATA_DIR=/tmp/junto-first cargo run -p junto-client-core --bin junto-cli -- notices list
JUNTO_DATA_DIR=/tmp/junto-first cargo run -p junto-client-core --bin junto-cli -- \
    notices set-settings channel:<channel id> --level all
JUNTO_DATA_DIR=/tmp/junto-first cargo run -p junto-client-core --bin junto-cli -- search "retention spec"
```

## Notifications and search between two instances

Start two instances as above. The first one is the owner, the second one a member.

- **Notices.** Write a message in the second instance that mentions the first person. The channel column of the first instance counts it at "Notifications"; the entry opens the list in the side panel. A row shows the place, the author and the start of the text, and a press on it shows the message in its channel and marks the notice as read. "Mark all as read" reads the list.
- **Level.** A right click on a channel (or the Menu key, or Shift and F10, while its row has the focus) opens its menu: "Notification settings…" has the level ("default", "all messages", "mentions only", "nothing") and the two switches for mentions of a role and of everyone. The menu of the server (the control beside the name of the workspace) has the same dialog for the server and for the workspace. An administrator of the server sets the level of a workspace for its members there.
- **Mute.** The menu of a channel mutes it for one hour, for eight hours, until tomorrow, or until it is turned off. The row of a direct conversation has the same menu.
- **Quiet times.** The settings (the gear beside the name) have "Do not disturb" and the daily quiet time. Both are stored on every server on which the instance has a session.
- **System notifications.** The application shows a notification of the system for a notice while its window is not the active one, or shows another place. On macOS that needs an application bundle, which the build does not make yet: an instance from `cargo run` shows none, and the list of the notices is where they are read.
- **Search.** Type words into the field of the header and press Enter. The side panel lists the messages and the lines of transcripts that hold the words, with filters for a channel, an author and a time span. A press on a message result shows the message in its place, and a press on a transcript result opens the transcript of its recording.

## Administration between two instances

The instance that claimed the server is its owner and may do everything. The menu of the server (the button beside the server name) has "Administration". The layer has a list of pages at the left; a page that the account may not use is not in the list. The second instance, a member, has the pages "Workspaces", "Archive and retention", "Members", "Access tokens" (its own tokens) and "Exports".

- **Workspaces.** "New workspace" makes a second workspace; it appears in the server book of the owner at once. "Rename" and "Delete" are in its row. The default workspace cannot be deleted. A member enters a further workspace with "Enter an invite code" and a code that names that workspace.
- **Channels.** The page lists the categories and the channels of the shown workspace. "New channel" under a category creates a text or a voice channel; "Up", "Down" and "Move to…" change its place; "Settings" opens the settings of the channel. The second instance shows each change without a reload.
- **One permission, one request.** A choice on "Permissions" of a channel sends the one permission that changed. Two administrators who change different permissions of one role at the same moment both keep their change.
- **Channel settings.** The gear in the header of a channel opens the same settings. "Overview" has the name, the category, "Permanent messages", "Private channel", and for a voice channel the user limit and "Hide who is inside". "Permissions" has, for each role, one choice per permission: allow, default, deny. A choice that would give a permission that the account does not hold in the channel is disabled, and the row says so. "Archive and deletion" has "Archive", "Export" and "Delete".
- **Archive.** Archive a channel while the second instance shows it: its header there gets the mark "Archived", and a bar in place of the composer says that nobody can write in it and when the server deletes it. The channel leaves the channel list of both instances. "Archive and retention" lists it with "Open" and "Restore". An administrator sets the archive period and the retention there.
- **Server.** For an administrator: the logo of the server. "Choose image…" takes a PNG, JPEG or WebP file of at most 256 KiB and 1024 by 1024 pixels. The second instance shows the logo in its server book at once, in place of the generated picture, and the generated picture again after "Remove logo". The logo is public: a person sees it before joining.
- **People.** "People" lists every account with "Remove" and "Ban". Each asks first, and takes an optional reason of at most 1,000 characters. The reason is kept in the audit entry and is not sent to the person. A ban ends the session of the second instance at once: its window shows the state "banned". "Unban" lets it log in again. "Members" lists the members of the shown workspace with their roles. An administrator has "Remove from workspace" there: the member loses that workspace and its roles in it, and keeps the account and the other workspaces. The owner, the own account and a member who stands at or above the administrator state in their row why they are not removed.
- **Notifications.** For an administrator, in the group of the shown workspace: the notification level that the workspace sets for members who chose none. "Not set" removes the level; the default of each member holds then. An account that holds a server role at or above the highest server role of the person states so in its row, and takes no removal, ban or unban.
- **Roles.** "Server roles" (for an administrator) and "Roles" of the shown workspace (for a person with "manage roles") list the roles from the highest to the lowest. "New role" creates one at the bottom. "Permissions" in a row shows one switch per permission; a switch that would give a permission that the person does not hold is disabled and says so. A role moves by its handle onto another row, or with "Up" and "Down". "Roles" in a row of "People" and of "Members" gives a role to that account or takes it. A role at or above the highest role of the person is locked, with its reason in the row. Give the second instance a role with "manage messages": its window offers "Delete" and "Pin" on the messages of the first instance at once, and no longer when the role is taken.
- **What a person may do.** The window offers an action when the server states its permission for the account: one list for each workspace, and one for each channel whose overrides change it. An override that denies "manage messages" to a role in one channel takes "Delete" and "Pin" from the holders of the role in that channel only.
- **Invites and join requests.** "New invite" asks for the expiry, the uses and the workspaces, and shows the code once, with "Copy". "Revoke" is in the row of an invite. "Join requests" lists the people who pressed "Request access", with "Approve" and "Deny".
- **Access tokens.** An administrator creates a token with a name, actions and an expiry. The token is shown once. The tool `junto-setup` uses such a token.
- **Exports.** "Export" in the settings of a channel, or the download button in the header of a direct conversation, asks for an export. "Exports" shows its state and saves the file to the download directory. The development server of `tools/dev-server.sh` keeps the files of exports in a temporary directory.
- **Audit log.** "Audit log" shows the entries of the server or of the shown workspace, the newest first, with "Load older". The filters are applied by the server: one action, the person who acted, the person concerned, and the last 24 hours, 7 days or 30 days. An entry of a ban or of a removal shows its reason.

The same routes from a terminal:

```sh
JUNTO_DATA_DIR=/tmp/junto-first cargo run -p junto-client-core --bin junto-cli -- workspaces list
JUNTO_DATA_DIR=/tmp/junto-first cargo run -p junto-client-core --bin junto-cli -- \
    channels create <workspace id> plans --kind text
JUNTO_DATA_DIR=/tmp/junto-first cargo run -p junto-client-core --bin junto-cli -- channels archive <channel id>
JUNTO_DATA_DIR=/tmp/junto-first cargo run -p junto-client-core --bin junto-cli -- accounts list
JUNTO_DATA_DIR=/tmp/junto-first cargo run -p junto-client-core --bin junto-cli -- \
    audit server --action account_banned --since 2030-01-01T00:00:00Z --limit 20
JUNTO_DATA_DIR=/tmp/junto-first cargo run -p junto-client-core --bin junto-cli -- \
    accounts ban <account id> --reason "Advertisements in every channel"
JUNTO_DATA_DIR=/tmp/junto-first cargo run -p junto-client-core --bin junto-cli -- \
    members remove <workspace id> <account id>
JUNTO_DATA_DIR=/tmp/junto-first cargo run -p junto-client-core --bin junto-cli -- roles list <workspace id>
JUNTO_DATA_DIR=/tmp/junto-first cargo run -p junto-client-core --bin junto-cli -- \
    roles move <workspace id> <role id> <position>
JUNTO_DATA_DIR=/tmp/junto-first cargo run -p junto-client-core --bin junto-cli -- logo set logo.png
```

`junto-cli` without arguments lists every command.

## A call on one machine

A call needs a media server. Start the development server with one:

```sh
tools/dev-server.sh --media
```

Start two instances with two data directories, join the server with both (see above), choose a voice channel in each and press "Join call". The first build of the application downloads the WebRTC library of the call implementation and takes several minutes.

What macOS asks for:

- **Microphone.** The first "Join call" makes macOS ask whether the microphone may be used. The question names the application that started Junto: the terminal, while Junto runs from `cargo run`. Press "Allow" once. The permission then holds for every instance that this terminal starts.
- When the answer was "Don't Allow", the call shows "Junto may not use the microphone": you hear the call and are not heard. Open System Settings, Privacy & Security, Microphone, turn the terminal on, and press "Try again" in the call. macOS can ask to restart the terminal.
- **Screen & System Audio Recording.** A share needs this permission. macOS has no "Allow" button for it: the person turns it on in System Settings. See the next section.

The Settings tab of the call shows the state of the permission (not asked yet, allowed, refused), the input level and the two devices.

Two instances on one machine share one microphone and one output:

- With loudspeakers, each instance plays what the other one sends and the microphone picks it up again. Use headphones.
- With headphones you hear your own voice with a delay, because the second instance plays what the first one sent. To hear one direction only, close the microphone of one instance (the microphone button beside the name) or turn its sound off (the headphones button).
- The level meters in the channel list, the edge of a tile and the line "Speaking" in the Members tab show that sound arrives, also with the sound off.

Echo cancellation, noise suppression, automatic gain and push-to-talk are in the app settings (the gear beside the name). Push-to-talk sends the microphone while the Option or the Control key is held and the window has the focus.

## Cameras and the detached window

The camera button starts and stops the chosen camera. The Settings tab of a voice call chooses the camera and stores the choice for calls. Permission is requested only after the person starts a camera. Refused permission, a restricted system, a disconnected device and interrupted capture have separate notices. The local preview is mirrored; pictures sent to other people are not. Clicking a camera tile focuses it, and the view setting can hide people without video.

Detach opens the current call in another window. Return or closing that window puts it back; Leave ends the call. Pin stores the always-on-top choice, and Fullscreen changes the call window. Its windowed position and size are stored in `call-window.json`. Native always-on-top behavior on Linux requires the platform integration described in ADR 0772.

The generated camera tests check received frame colors, capture shutdown, reconnect and independence from microphone blocks. The detached-window tests exercise controls and shared call ownership on the test platform. These tests do not open a camera or verify native window stacking or a permission dialog.

## Sharing a window with sound between two instances

These permission and source-list steps describe macOS. Linux uses the desktop chooser described in the Linux section below. Both instances are in one call (see above). In the first instance:

1. Press the share button of the call strip (the screen icon between the camera and the hand). The dialog "Share your screen" opens.
2. Without the permission of macOS the dialog says "Junto may not record the screen yet". Press "Open System Settings": macOS lists the application that started Junto under Privacy & Security, Screen & System Audio Recording. That is the terminal while Junto runs from `cargo run`. Turn it on. macOS can ask to quit and reopen that application; start both instances again afterwards. A terminal that has the permission already skips this step. "Check again" reads the permission again.
3. Choose a screen or a window under "What to share".
4. Choose the sound under "Sound of this share": "No sound", "All system sound", or "This application" for a window ("One application" for a screen, with the application chosen from the list under it). Nothing is chosen for you, and "Share" works only after both choices.
5. Press "Share". The share button is lit while the share runs; a press on it ends the share.

The second instance shows the picture in the stage of the call, with the line "… is sharing" and the sound choice, and plays the sound. When both instances share, a bar lists the two shares; a press on a card watches that share, and the sound follows the choice. The Settings tab of the call has the resolution and the frame rate of the next share, and "Sound of the share you watch" (0 % keeps the picture without its sound).

What to expect on one machine:

- **A source of sound.** Share a window of an application that plays sound, such as a video in a browser or a music player, with "This application". The sharer does not need to hear it: the capture does not depend on the output volume.
- **Junto is not in the lists.** No window of Junto and no second instance of Junto is offered, and the sound of Junto is never part of a share (ADR 0502). Both instances run from one terminal, so macOS treats them as one application for this rule: with "All system sound" the first instance shares every sound of the machine except what the two instances play.
- **You hear the shared sound twice with loudspeakers or one pair of headphones**: once from the application that plays it, and once from the second instance, about 0.3 seconds later. Turn the sound of the second instance off (the headphones button) to check the picture alone, or lower the volume of the application to hear only what the call carries.
- **The sound of a share has one channel.** The call library sends one channel per sound track (ADR 0737).
- **A blocked microphone closes the sound of a share.** The picture stays, and the line of the share says so (ADR 0510).
- The picture of a screen that shows the first instance contains that instance, and its preview repeats inside itself.

## The rules of a room between two instances

Both instances are on one server; the one that claimed the server is its owner and may do everything, the other is a member with the permissions of the "everyone" role ("lock room" and "run meeting mode" on a new server).

- **Hand.** The hand button of the call strip raises and lowers the hand. The mark appears beside the name in the channel list, on the tile and in Members.
- **Meeting mode.** The instance that shares presses "Meeting mode" in the header of the call. A press in the first second of a share can be refused, and the notice says so. The other instance shows the bar "Meeting mode is on" and its microphone as closed by the meeting. It raises the hand; the presenter sees "… raised a hand" and presses "Allow"; the other instance shows "You may speak" and opens the microphone with "Unmute". When the presenter turns meeting mode off or stops sharing, an instance whose microphone was still blocked and not muted shows the notice that its microphone is open again.
- **Lock, user limit, hidden occupants.** "Lock room" is in the header and in the Settings tab of the call under "Room"; the user limit and "Hide who is inside" are there for a person with "manage channel". A call keeps the hide setting that the room had when the call began; a change holds from the next call, and the row of the setting says so. With the room locked, the instance outside selects the voice channel, presses "Ask to enter" in the channel list, and the instance inside answers the notice with "Let in" or "Deny".
- **Invitation.** Members tab of the call, "Invite a member", "Invite". The other instance shows the notice with "Join" and "Decline". An invitation passes the lock and the limit.
- **Menu of a person.** A right click on a person in a voice channel of the channel list (or Shift and F10 while the row has the focus). The owner sees "Move to…", "Mute for everyone", the sound block and "Disconnect"; a member sees "Invite to my room" and the volume. A move needs a second voice channel.

## Recording

The header of the voice channel of the call has "Record" for a person who holds "record calls" in the voice channel. A new server gives that permission to no role: the owner sees the button, and a member sees it disabled with the reason. The development server of `tools/dev-server.sh` has no recorder, so "Record" there answers with the notice that the server has no recorder. With a recorder the header shows "REC" with the time, "visible to everyone" and "Stop", and a recording that fails shows "Recording failed" with "Details".

A finished recording is a card in the chat of its voice channel, for every person who reads that chat and is not in the call on this device:

- **Card.** The channel and the day, the times, the length, the person who started the recording, the number of microphone files, the state of the transcript, and the first lines of the transcript with the speaker of each line. A recording that the server still works on, that lost a part, or that saved nothing says so.
- **Open.** "Open" saves the video to the download directory and gives it to the video player of the system. The application has no player.
- **Download.** "Download" lists the video, the audio file of each person and the transcript as text. A saved file is listed in the card with the control that shows it in its folder. A download continues after a cut connection.
- **Transcript.** "Show all" opens the transcript in the side panel, with "Show more lines" for a long one.
- **Remove.** "Remove" is active for a person who holds "manage recordings" in the channel, and asks first.

The same routes from a terminal:

```sh
JUNTO_DATA_DIR=/tmp/junto-first cargo run -p junto-client-core --bin junto-cli -- recordings list <workspace id>
JUNTO_DATA_DIR=/tmp/junto-first cargo run -p junto-client-core --bin junto-cli -- recordings lines <recording id>
JUNTO_DATA_DIR=/tmp/junto-first cargo run -p junto-client-core --bin junto-cli -- recordings show <recording id>
JUNTO_DATA_DIR=/tmp/junto-first cargo run -p junto-client-core --bin junto-cli -- \
    recordings download <recording id> <segment id> --to /tmp/junto-files
```

`junto-cli` without arguments lists the other actions of `recordings`, with the correction of a line and the name of a speaker.

## Options

| Option | Meaning |
|---|---|
| `--data-dir <directory>` | The data directory. Default: the variable `JUNTO_DATA_DIR`, then the directory of the user (ADR 0710). |
| `--scale <percent>` | The interface scale of this start, from 90 to 150. The stored setting stays. |
| `--no-activate` | Open the window without bringing it to the front. |
| `--check-startup` | Exit successfully after the first completed window frame; also disables activation. |

The variable `JUNTO_LOG_LEVEL` sets the level of the log on standard error (`warn` by default). On Windows `junto.exe` is a program of the Windows subsystem: the log of a running window is kept only when standard error is redirected (`junto.exe 2> junto.log`), and a command that only writes text (`--help`, `--version`, an error before the window) writes it into the console of the terminal that started it (ADR 0755).

## Tests

```sh
tools/with-test-postgres.sh tools/serial.sh cargo test -p junto-client
```

The tests in `tests/app` open the window code on gpui's test platform, with a client on a data directory of its own, against the server binary of this checkout. They click what is drawn and read what the window shows. A second client without a window is the administrator of the server. `tests/app/messages.rs` writes, answers, edits and deletes messages, reacts, sends and saves a file, follows the unread signs, follows a quoted message that changes, and checks that the text of one account is not shown to the next account of the server. `tests/app/servers.rs` joins a second server by approval, follows the unread signs of a server in the background, orders the server book, and shows the update notice from a source of the test. `tests/app/account.rs` and `tests/app/linking.rs` change the profile, rename and remove devices, and link a second window on one server and on two servers. `tests/app/people.rs` follows the picture of another person. `tests/app/logo.rs` sets and removes the logo of a server and follows it in the window of a member. `tests/app/remaining.rs` removes a member from one workspace with a reason and follows the window of that member, checks the rank rule of that removal in the row before a request, sets and removes the notification level of a workspace, and sets a quiet time in several named time zones and checks that the server holds a notice back at the minute that the client computed. `tests/app/threads.rs` follows a thread and stops following it, answers in a thread and follows its count and its unread sign, asks for a fork with each refusal and with success, pins and unpins a message, writes in a direct conversation, and checks the marker of permanent messages. `tests/app/notices.rs` follows a mention to its notice and to the notification that the system would get, with the rules for an active window and for a quiet time, jumps from a notice to its message, marks all as read, sets the level and the mute of a channel, and checks that the notices of a channel leave the list with the access to it. `tests/app/search.rs` finds messages and a line of a transcript, uses the filters, jumps to a result, reads a further page, and checks that the results of a channel leave the panel with the access to it. No test shows a notification of the system: the tests give the application an interface that keeps what it was told.

`tests/app/recordings.rs` runs the server of this checkout inside the test process with its recorder in memory, because the server binary has no such recorder. It makes a recording, lets the server process the files with `ffmpeg` and `ffprobe` (both must be installed), and checks the card, the saved files against the files of the server, the transcript, the result of a search, and the removal.

No test opens a microphone or camera, plays a sound or captures a screen: the microphone of each client is a tone, its output is a sink that measures what would be played, and its screen capture is a picture of two colours with a tone on each channel. No test causes a permission question of the system. The tests of a call need a LiveKit server and are ignored in the plain run:

```sh
tools/with-test-postgres.sh tools/with-test-livekit.sh tools/serial.sh \
    cargo test -p junto-client --test app live:: -- --ignored
tools/with-test-postgres.sh tools/with-test-livekit.sh tools/serial.sh \
    cargo test -p junto-media --test calls -- --ignored
```

The first command runs the window against a real server and a real media server: a call, a share, the request for a recording, the read position of the chat of a call, and the rules of a room (a locked, full and hidden room with the request to enter, meeting mode as participant and as presenter, invitations, and the menu of a person with the blocks, a move and a removal). The second command runs the tests of the media engine: two clients that hear each other, mute, a microphone block, the devices, the microphone permission, one call over two servers, a leave while a join or a new token is on its way, the return of a call after the media server was restarted, and the shares: a share with its picture and its sound, two shares and the choice of the viewer, full-resolution share reception and the conversion bounds of the final drawing size, a share that ends by itself, the blocks of the server, a move between voice channels, a person whom the presenter of meeting mode allows to speak, and a person who enters a voice channel that hides who is inside.

The calls to ScreenCaptureKit are not run by a test. The parts of the macOS capture that decide something (which application is Junto, which windows are listed, how a sound buffer is read) are functions with tests; the first real share is a test by a person.

## Screens as pictures

`junto-screens` draws every screen and state without a window and writes one PNG file per screen. Nothing appears on the display.

```sh
cargo run -p junto-client --features test-support --bin junto-screens -- <directory>
```

The variable `JUNTO_SCREENS_ONLY` limits the run to one set: `shares`, `rules`, `messages`, `threads`, `discovery` (notices, notification settings, search) `recorded` (the card of a recording and the transcript), or `cameras` (camera states, direct calls and the detached window). The set `frames` writes no picture: it draws the window with 50 and with 10,000 messages and prints the time of a frame without a change, after a scroll, and after a new message (ADR 0740). Run it with `--release` for times that say something about the application. The set `audit-frames` measures frames of the audit log with 50 and 2,000 entries: without a change, after a wheel scroll, after a jump of the scroll bar and after a newer entry, with five warm-up frames and 60 measured frames per case (ADR 0761).

## Layout of the crate

| Path | Content |
|---|---|
| `src/theme.rs` | Every colour, font, type size, radius and fixed measure, and the interface scale. |
| `src/text.rs` | The catalog of the interface text, with the sentence for each failure of a flow. |
| `src/assets.rs`, `assets/` | The fonts and the icons, carried in the binary. The fonts are under the SIL Open Font License; the licence texts are beside the files. |
| `build.rs`, `src/window_icon.rs` | The application icon in the program: the icon group of the Windows program, and the picture that the windows give to an X11 window manager (ADR 0756). |
| `src/ui/` | The components. |
| `src/state/` | The state of the application over the client core, and the derivation of what the window shows. |
| `src/compose.rs` | The marks of the composer and the document of a message, in both directions, and the limits of a document. |
| `src/state/feed.rs` | The derivation of the rows of a message list. |
| `src/state/notices.rs`, `src/state/search.rs`, `src/state/recorded.rs` | The derivation of the notices with the notification settings and the system notifications, of the search panel, and of the card of a recording with its transcript. |
| `src/notify.rs` | The interface to the notifications of the system, and on Windows the identity of the notifications (ADR 0742). |
| `src/shell/title_bar.rs` | The title bar: the headers that act as the title bar on macOS, and the strip with the window buttons on Windows and on a Wayland compositor without server decorations (ADR 0703). |
| `src/highlight.rs` | The syntax colours of a code block. |
| `src/shell/` | The main window. `call.rs` draws the call, `share.rs` the dialog and the stage of a share, `room.rs` the rules of the room of a call, `feed.rs` the message list, `composer.rs` the composer with its pickers, `notices.rs` the notices and the notification settings, `search.rs` the search, and `recorded.rs` the card of a recording and the transcript. |
| `src/state/admin.rs`, `src/state/admin/` | The administration: the open page, its requests, and the derivation of each page. |
| `src/shell/admin.rs`, `src/shell/admin/` | The layer of the administration and its pages. |
| `src/text/admin.rs`, `src/text/admin/` | The text of the administration. |
| `src/state/book.rs`, `src/shell/book.rs` | The order of the server book, the unread sign of a server, and the update notice. |
| `src/state/account.rs`, `src/shell/account.rs`, `src/shell/account/` | The account layer: profile and devices. |
| `src/state/linking.rs`, `src/shell/linking.rs` | The link of a device on one server and on all servers. |
| `src/state/people.rs`, `src/shell/people.rs`, `src/ui/avatar.rs` | The profiles and the avatar images of the people that the window shows. |
| `src/state/logo.rs` | The logo image of each server that the server book and the administration show. |
| `src/state/followed.rs` | The follow state of the open thread, the followed threads of the shown channel, and the roots that are read for their titles. |
| `src/zones.rs` | The names of the time zones, the zone of the system, and the clock of a named zone. |
| `src/bin/junto-screens.rs`, `src/bin/junto-screens/` | The tool that draws the screens. |

## Call movement and startup checks

Drag another person from a voice channel onto a voice channel. With the move permission, this requests a move; otherwise the destination must be the current call and the action sends an invitation. The drop is checked against the current state. The server decides access and permission in both places, and a refusal is shown without changing the source call. Focus a person and press Shift+F10 for the same Invite or Move to actions, then use arrow keys and Enter.

The detached call follows token renewal and moderator moves. Its title includes the call and server. Closing the main window also closes the detached window; Return keeps the call connected. Window placement is stored against the display UUID, with consecutive changes coalesced into the latest placement.

A bare macOS executable does not register system notifications: these require an application bundle. Settings explains their unavailability. Permission usage descriptions remain embedded for camera and microphone actions. A native startup check uses an empty data directory and exits after a completed frame:

```sh
tools/serial.sh sh tools/check-client-start.sh <empty-data-directory>
```

This check opens a native window without activation. It is separate from the headless window tier and does not verify camera hardware, permission dialogs, native fullscreen or window ordering.

## Building and checking Linux in containers

`tools/linux-container.sh` builds the Linux toolchain image and runs a command with private PostgreSQL, LiveKit, D-Bus and PipeWire services. Use a separate launcher invocation for each full suite repetition so test databases do not accumulate between runs. The retained target volume is for one architecture and the debug profile. The image has arm64 and amd64 definitions; tests on one architecture do not establish results on the other. The shared lock script must provide its Linux `flock` path when the launcher runs on a Linux host. The launcher itself delegates to that script.

```sh
tools/linux-container.sh cargo check --workspace --all-targets
tools/linux-container.sh cargo test --workspace --no-fail-fast
tools/linux-container.sh cargo clippy --workspace --all-targets -- -D warnings
tools/linux-container.sh cargo fmt --all -- --check
```

The window code can run on its test platform under a private X11 display or a headless Wayland compositor:

```sh
tools/linux-container.sh tools/linux/display.sh x11 \
    cargo test -p junto-client --features test-support --test app
tools/linux-container.sh tools/linux/display.sh wayland \
    cargo test -p junto-client --features test-support --test app
tools/linux-container.sh tools/linux/display.sh wayland \
    cargo test -p junto-client --features test-support --test app live:: -- --ignored
```

The first two commands run ordinary window tests. The third runs calls and shares through the real test services, while their pictures, microphone and output remain synthetic. None uses a host microphone, speaker, display or permission dialog.

To export the rendered screens:

```sh
JUNTO_LINUX_ARTIFACTS=/tmp/junto-linux-screens \
    tools/linux-container.sh tools/linux/display.sh wayland \
    cargo run -p junto-client --features test-support --bin junto-screens -- \
    /tmp/junto-linux-artifacts
```

`tools/linux/window-smoke.sh` starts the real binary on a display created by `display.sh` and checks for a painted application surface. Build `junto` first in the same target volume. A successful gpui test-platform run alone does not prove that the real application presents a frame. The image uses Mesa 25.0.7 and the smoke test uses normal renderer selection. The smoke test repeats its check of the painted surface for up to 30 seconds. `JUNTO_LINUX_TEST_RENDERER=opengl` remains an explicit diagnostic override. These software-renderer results do not cover a hardware GPU driver.

The software Vulkan driver of Mesa 22.3.6 presents a black window under X11. Before its first window under X11 the binary reads the Vulkan adapters, and it starts itself again without Vulkan drivers when that driver of a Mesa older than 25 is the adapter that gpui would use; gpui then draws with OpenGL (ADR 0705, `src/renderer.rs`). The image keeps that driver for this check:

```sh
tools/linux-container.sh sh -c 'cargo build -p junto-client --bin junto &&
    JUNTO_LINUX_TEST_RENDERER=mesa22 tools/linux/display.sh x11 \
    tools/linux/window-smoke.sh /tmp/junto-linux-artifacts/mesa22'
```

With `JUNTO_RENDERER_CHECKED=1` in the environment the binary skips its check, and this command then fails with a black window.

`window_level::keep_above` is the interface by which a detached call window asks to stay above other windows (ADR 0507). The detached call window calls it when it opens with the choice "keep on top" and when the person changes the choice. On Linux the window is not opened again for the choice, and the choice is drawn only under X11. Under X11 it puts the request for the state "above" into a queue, and a thread of its own sends the requests to the window manager in their order, so the thread of the window does not wait for the display server. It returns nothing: the outcome, also for a Wayland window that has no such request, is written to the log. Its tests need the private X11 display and its window manager:

```sh
tools/linux-container.sh tools/linux/display.sh x11 \
    cargo test -p junto-client --lib window_level -- --ignored
```

The built-in fonts are used on both systems. The Linux image also supplies a colour emoji font. Window decorations, font rasterisation and colour rounding can differ between backends. Screenshot comparisons must distinguish those differences from missing controls or content.

Linux call sound uses PipeWire directly; see [`junto-media`](../media/README.md). The share picker offers desktop screen/window selection and the available sound choices under ADR 0751. The desktop authorises each share after Share is pressed; a window selection does not implicitly choose an application's sound. Container checks cover native shared sound and portal screen pixels with the explicitly patched test backend described in the media README; a human chooser and an unmodified desktop backend remain unverified.

Linux system notifications use the session bus's `org.freedesktop.Notifications` service behind the same policy interface as macOS. The private-service tests check replacement, withdrawal, activation, the markup capability, a service restart, a replaced session bus and an alert that waited too long, without showing a desktop notification:

```sh
tools/linux-container.sh cargo test -p junto-client --lib notify::linux::tests
```

Application tests and the screenshot tool retain the in-memory notifier. A real desktop daemon's visual presentation and a person's portal interaction require separate manual checks; fake-service and headless tests do not establish those results.

## macOS bundles and updates

See [the macOS installation instructions](../../docs/client-macos.md) and [release tooling](../../packaging/README.md). A configured bundle checks the build-time release source, verifies signed metadata and downloads, and installs only a newer bundle with the same code-signing identity. The update notice and Version settings offer the explicit install action; after success, quit and reopen Junto. Without a source, Version states that none is configured. Recovery verifies and restores the retained archive; the documented standalone command works even when the new executable cannot start. `--rollback-update` invokes the same checks from a working application and exits without a window.
