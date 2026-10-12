# junto-client

The Junto desktop application: a gpui window on `junto-client-core` and `junto-media`. The binary is `junto`. The design decisions are in [`docs/adr/`](../../docs/adr/README.md): ADR 0730 describes this crate, ADR 0731 its theme and components, ADR 0732 joining a server in the window, ADR 0733 the media engine, ADR 0734 the call of a client, ADR 0735 the screens of a voice call, ADR 0736 the screen capture, ADR 0737 shares in the media engine, ADR 0738 the screens of a share and of a recording, ADR 0739 the rules of a call, ADR 0740 the message list and the composer, ADR 0741 the thread panel, pins, permanent messages and direct conversations, ADR 0742 the notices, the notification settings and the system notifications, ADR 0743 the search, and ADR 0744 the card of a recording and the transcript.

This version shows the servers of the server book, their workspaces and channels, the people in voice channels, and the state of each server. It joins a server with a code or a request for access, and it holds a voice call in a voice channel: microphone, sound, mute, a volume per person, levels, push-to-talk and the choice of the sound devices. In a call a person shares a screen or a window with its sound, watches the shares of the others, starts and stops a recording, and uses the rules of the room: meeting mode with raised hands, the lock, the user limit, requests to enter, invitations, the menu of a person with blocks, removal and move. A person reads and writes messages in a text channel, in the chat of a voice channel and in a direct conversation: formatted text, replies, mentions, reactions, files, edits and deletions, with unread signs in the channel list. A person answers in the thread of a message, forks a thread into a channel, and pins messages. The application lists the notices of a person, takes the notification level of a server, a workspace and a channel, the mute of a place and the quiet times, and shows notifications of the system. It searches messages and transcripts. A finished recording is a card in the chat of its voice channel, with its files and its transcript. It supports cameras, direct-message calls and a detached call window. It has no list of the recordings of a workspace and no correction of a transcript. It is built and tested on macOS and builds for Linux, where the screen capture is not built yet; Windows is not supported yet.

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
- **What is new in a thread.** Close the panel in the first instance and answer in the thread with the second one. The first instance shows the count of the new replies on the message, and lists the thread under its channel in the channel column. With the default notification level, the name of the channel gets a sign only for a mention.
- **Fork.** The panel has "Fork thread into…" with "Text channel" and "Voice channel" for a person whose roles hold "manage channel"; the owner of a new server may do it, a member sees the two controls disabled. The dialog takes a name. The new channel holds a copy of the message and of its replies, and the panel then names the channel in place of the two controls. A second fork of the same thread is refused, and the dialog says so.
- **Pins.** "Pin" is in the bar and in the menu of a message for a person whose roles hold "manage messages". "Pins" in the header opens the list of the channel, with "Go to message" and "Unpin".
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

Both instances are in one call (see above). In the first instance:

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
- **Lock, user limit, hidden occupants.** "Lock room" is in the header and in the Settings tab of the call under "Room"; the user limit and "Hide who is inside" are there for a person with "manage channel". With the room locked, the instance outside selects the voice channel, presses "Ask to enter" in the channel list, and the instance inside answers the notice with "Let in" or "Deny".
- **Invitation.** Members tab of the call, "Invite a member", "Invite". The other instance shows the notice with "Join" and "Decline". An invitation passes the lock and the limit.
- **Menu of a person.** A right click on a person in a voice channel of the channel list (or Shift and F10 while the row has the focus). The owner sees "Move to…", "Mute for everyone", the sound block and "Disconnect"; a member sees "Invite to my room" and the volume. A move needs a second voice channel.

## Recording

The header of the voice channel of the call has "Record" for a person whose roles hold "record calls". A new server gives that permission to no role: the owner sees the button, and a member sees it disabled with the reason. The development server of `tools/dev-server.sh` has no recorder, so "Record" there answers with the notice that the server has no recorder. With a recorder the header shows "REC" with the time, "visible to everyone" and "Stop", and a recording that fails shows "Recording failed" with "Details".

A finished recording is a card in the chat of its voice channel, for every person who reads that chat and is not in the call on this device:

- **Card.** The channel and the day, the times, the length, the person who started the recording, the number of microphone files, the state of the transcript, and the first lines of the transcript with the speaker of each line. A recording that the server still works on, that lost a part, or that saved nothing says so.
- **Open.** "Open" saves the video to the download directory and gives it to the video player of the system. The application has no player.
- **Download.** "Download" lists the video, the audio file of each person and the transcript as text. A saved file is listed in the card with the control that shows it in its folder. A download continues after a cut connection.
- **Transcript.** "Show all" opens the transcript in the side panel, with "Show more lines" for a long one.
- **Remove.** "Remove" is active for a person whose roles hold "manage recordings", and asks first.

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

The variable `JUNTO_LOG_LEVEL` sets the level of the log on standard error (`warn` by default).

## Tests

```sh
tools/with-test-postgres.sh tools/serial.sh cargo test -p junto-client
```

The tests in `tests/app` open the window code on gpui's test platform, with a client on a data directory of its own, against the server binary of this checkout. They click what is drawn and read what the window shows. A second client without a window is the administrator of the server. `tests/app/messages.rs` writes, answers, edits and deletes messages, reacts, sends and saves a file, follows the unread signs, follows a quoted message that changes, and checks that the text of one account is not shown to the next account of the server. `tests/app/threads.rs` answers in a thread and follows its count and its unread sign, asks for a fork with each refusal and with success, pins and unpins a message, writes in a direct conversation, and checks the marker of permanent messages. `tests/app/notices.rs` follows a mention to its notice and to the notification that the system would get, with the rules for an active window and for a quiet time, jumps from a notice to its message, marks all as read, sets the level and the mute of a channel, and checks that the notices of a channel leave the list with the access to it. `tests/app/search.rs` finds messages and a line of a transcript, uses the filters, jumps to a result, reads a further page, and checks that the results of a channel leave the panel with the access to it. No test shows a notification of the system: the tests give the application an interface that keeps what it was told.

`tests/app/recordings.rs` runs the server of this checkout inside the test process with its recorder in memory, because the server binary has no such recorder. It makes a recording, lets the server process the files with `ffmpeg` and `ffprobe` (both must be installed), and checks the card, the saved files against the files of the server, the transcript, the result of a search, and the removal.

No test opens a microphone or camera, plays a sound or captures a screen: the microphone of each client is a tone, its output is a sink that measures what would be played, and its screen capture is a picture of two colours with a tone on each channel. No test causes a permission question of the system. The tests of a call need a LiveKit server and are ignored in the plain run:

```sh
tools/with-test-postgres.sh tools/with-test-livekit.sh tools/serial.sh \
    cargo test -p junto-client --test app live:: -- --ignored
tools/with-test-postgres.sh tools/with-test-livekit.sh tools/serial.sh \
    cargo test -p junto-media --test calls -- --ignored
```

The first command runs the window against a real server and a real media server: a call, a share, the request for a recording, the read position of the chat of a call, and the rules of a room (a locked, full and hidden room with the request to enter, meeting mode as participant and as presenter, invitations, and the menu of a person with the blocks, a move and a removal). The second command runs the tests of the media engine: two clients that hear each other, mute, a microphone block, the devices, the microphone permission, one call over two servers, a leave while a join or a new token is on its way, the return of a call after the media server was restarted, and the shares: a share with its picture and its sound, two shares and the choice of the viewer, full-resolution share reception and the conversion bounds of the final drawing size, a share that ends by itself, the blocks of the server, a move between voice channels, and a person whom the presenter of meeting mode allows to speak.

The calls to ScreenCaptureKit are not run by a test. The parts of the macOS capture that decide something (which application is Junto, which windows are listed, how a sound buffer is read) are functions with tests; the first real share is a test by a person.

## Screens as pictures

`junto-screens` draws every screen and state without a window and writes one PNG file per screen. Nothing appears on the display.

```sh
cargo run -p junto-client --features test-support --bin junto-screens -- <directory>
```

The variable `JUNTO_SCREENS_ONLY` limits the run to one set: `shares`, `rules`, `messages`, `threads`, `discovery` (notices, notification settings, search) `recorded` (the card of a recording and the transcript), or `cameras` (camera states, direct calls and the detached window). The set `frames` writes no picture: it draws the window with 50 and with 10,000 messages and prints the time of a frame without a change, after a scroll, and after a new message (ADR 0740). Run it with `--release` for times that say something about the application.

## Layout of the crate

| Path | Content |
|---|---|
| `src/theme.rs` | Every colour, font, type size, radius and fixed measure, and the interface scale. |
| `src/text.rs` | The catalog of the interface text, with the sentence for each failure of a flow. |
| `src/assets.rs`, `assets/` | The fonts and the icons, carried in the binary. The fonts are under the SIL Open Font License; the licence texts are beside the files. |
| `src/ui/` | The components. |
| `src/state/` | The state of the application over the client core, and the derivation of what the window shows. |
| `src/compose.rs` | The marks of the composer and the document of a message, in both directions, and the limits of a document. |
| `src/state/feed.rs` | The derivation of the rows of a message list. |
| `src/state/notices.rs`, `src/state/search.rs`, `src/state/recorded.rs` | The derivation of the notices with the notification settings and the system notifications, of the search panel, and of the card of a recording with its transcript. |
| `src/notify.rs` | The interface to the notifications of the system. |
| `src/highlight.rs` | The syntax colours of a code block. |
| `src/shell/` | The main window. `call.rs` draws the call, `share.rs` the dialog and the stage of a share, `room.rs` the rules of the room of a call, `feed.rs` the message list, `composer.rs` the composer with its pickers, `notices.rs` the notices and the notification settings, `search.rs` the search, and `recorded.rs` the card of a recording and the transcript. |
| `src/bin/junto-screens.rs`, `src/bin/junto-screens/` | The tool that draws the screens. |


## Call movement and startup checks

Drag another person from a voice channel onto a voice channel. With the move permission, this requests a move; otherwise the destination must be the current call and the action sends an invitation. The drop is checked against the current state. The server decides access and permission in both places, and a refusal is shown without changing the source call. Focus a person and press Shift+F10 for the same Invite or Move to actions, then use arrow keys and Enter.

The detached call follows token renewal and moderator moves. Its title includes the call and server. Closing the main window also closes the detached window; Return keeps the call connected. Window placement is stored against the display UUID, with consecutive changes coalesced into the latest placement.

A bare macOS executable does not register system notifications: these require an application bundle. Settings explains their unavailability. Permission usage descriptions remain embedded for camera and microphone actions. A native startup check uses an empty data directory and exits after a completed frame:

```sh
tools/serial.sh sh tools/check-client-start.sh <empty-data-directory>
```

This check opens a native window without activation. It is separate from the headless window tier and does not verify camera hardware, permission dialogs, native fullscreen or window ordering.
