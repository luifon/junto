# junto-client

The Junto desktop application: a gpui window on `junto-client-core` and `junto-media`. The binary is `junto`. The design decisions are in [`docs/adr/`](../../docs/adr/README.md): ADR 0730 describes this crate, ADR 0731 its theme and components, ADR 0732 joining a server in the window, ADR 0733 the media engine, ADR 0734 the call of a client, ADR 0735 the screens of a voice call, ADR 0736 the screen capture, ADR 0737 shares in the media engine, ADR 0738 the screens of a share and of a recording, ADR 0739 the rules of a call, and ADR 0740 the message list and the composer.

This version shows the servers of the server book, their workspaces and channels, the people in voice channels, and the state of each server. It joins a server with a code or a request for access, and it holds a voice call in a voice channel: microphone, sound, mute, a volume per person, levels, push-to-talk and the choice of the sound devices. In a call a person shares a screen or a window with its sound, watches the shares of the others, starts and stops a recording, and uses the rules of the room: meeting mode with raised hands, the lock, the user limit, requests to enter, invitations, the menu of a person with blocks, removal and move. A person reads and writes messages in a text channel and in the chat of a voice channel: formatted text, replies, mentions, reactions, files, edits and deletions, with unread signs in the channel list. It has no direct conversations in the window, no threads, no search, no camera and no direct-message calls. It is built and tested on macOS and builds for Linux, where the screen capture is not built yet; Windows is not supported yet.

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

## Options

| Option | Meaning |
|---|---|
| `--data-dir <directory>` | The data directory. Default: the variable `JUNTO_DATA_DIR`, then the directory of the user (ADR 0710). |
| `--scale <percent>` | The interface scale of this start, from 90 to 150. The stored setting stays. |
| `--no-activate` | Open the window without bringing it to the front. |

The variable `JUNTO_LOG_LEVEL` sets the level of the log on standard error (`warn` by default).

## Tests

```sh
tools/with-test-postgres.sh tools/serial.sh cargo test -p junto-client
```

The tests in `tests/app` open the window code on gpui's test platform, with a client on a data directory of its own, against the server binary of this checkout. They click what is drawn and read what the window shows. A second client without a window is the administrator of the server. `tests/app/messages.rs` writes, answers, edits and deletes messages, reacts, sends and saves a file, follows the unread signs, follows a quoted message that changes, and checks that the text of one account is not shown to the next account of the server.

No test opens a microphone, plays a sound or captures a screen: the microphone of each client is a tone, its output is a sink that measures what would be played, and its screen capture is a picture of two colours with a tone on each channel. No test causes a permission question of the system. The tests of a call need a LiveKit server and are ignored in the plain run:

```sh
tools/with-test-postgres.sh tools/with-test-livekit.sh tools/serial.sh \
    cargo test -p junto-client --test app live:: -- --ignored
tools/with-test-postgres.sh tools/with-test-livekit.sh tools/serial.sh \
    cargo test -p junto-media --test calls -- --ignored
```

The first command runs the window against a real server and a real media server: a call, a share, the request for a recording, the read position of the chat of a call, and the rules of a room (a locked, full and hidden room with the request to enter, meeting mode as participant and as presenter, invitations, and the menu of a person with the blocks, a move and a removal). The second command runs the tests of the media engine: two clients that hear each other, mute, a microphone block, the devices, the microphone permission, one call over two servers, a leave while a join or a new token is on its way, the return of a call after the media server was restarted, and the shares: a share with its picture and its sound, two shares and the choice of the viewer, the layer that the media server sends for a drawn size, a share that ends by itself, the blocks of the server, a move between voice channels, and a person whom the presenter of meeting mode allows to speak.

The calls to ScreenCaptureKit are not run by a test. The parts of the macOS capture that decide something (which application is Junto, which windows are listed, how a sound buffer is read) are functions with tests; the first real share is a test by a person.

## Screens as pictures

`junto-screens` draws every screen and state without a window and writes one PNG file per screen. Nothing appears on the display.

```sh
cargo run -p junto-client --features test-support --bin junto-screens -- <directory>
```

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
| `src/shell/` | The main window. `call.rs` draws the call, `share.rs` the dialog and the stage of a share, `room.rs` the rules of the room of a call, `feed.rs` the message list, and `composer.rs` the composer with its pickers. |
| `src/bin/junto-screens.rs`, `src/bin/junto-screens/` | The tool that draws the screens. |
