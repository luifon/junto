# 0740. The message list and the composer in the window

## Status

Accepted

## Context

The approved design draws a text channel with its messages (view A), the chat of a voice channel (view G) and the chat tab of a call (view B). ADR 0401 and ADR 0408 define what a message holds: a typed document, replies, reactions, files, system lines. ADR 0721 gives the window a timeline per open place, the waiting messages and the unread summaries. gpui has a list element that measures a row when it is drawn, and the component library has a text area with inline tokens. The design draws no state for a message that waits, no control for the actions of a message, and no direct conversations.

## Decision

Derivation:

- A function without side effects (ADR 0730) derives the rows of a list from a timeline: day separators, the line of the first unread message, messages, system lines, the waiting messages, and a row at each end that states whether the history goes on.
- Messages of one author on one day form a group while each follows the one before within 5 minutes. A reply, a system line and a deleted message end a group. A message of a group is drawn without the avatar and the name.
- Times and days are shown in the time zone of the system. A moment that the clock of that zone cannot state is shown as the server wrote it (ADR 0747).
- The line of the first unread message stands before the first message after the read position that another person wrote. It stays where it was when the place was opened, until the person leaves the place. No line is drawn when every loaded message is unread.
- A reply shows the author and the start of the message that it answers. A message that the list does not hold is read from the server, at most 8 at one time. It is read again when an event names it and when the state was reloaded, so the reply shows an edit and a deletion. The window keeps the quoted messages that the replies of the shown place name, and no others, up to the weight that ADR 0747 states.

List:

- The list is the `list` element of gpui, aligned to its end. Rows are compared with the rows before by the copy of the message that the state holds; the rows that changed are spliced, so the other rows keep their measured heights and the list keeps its position when a page is added before or after.
- Within 4 rows of an end the next page is read. The window tells the client core where the list stands, for a reload (ADR 0721). The window keeps a record of what it told, and sends a position only when it differs from the record. A read of the latest messages makes the client core follow the end, so the record becomes the end too; and a place that the window opens again in the client core gets the recorded position with the open. Reason: the record kept the position from before such a read or such an open, so a list that came back to that position was not told again, and the client core read the end at the next reload. Test: `the_position_of_a_list_is_told_again_after_a_read_of_the_latest_messages` in `crates/client`.
- The list follows new messages while it shows its end. A button leads to the end when the list is elsewhere.
- Formatted text is one text layout per block with runs: bold, italic, struck, inline code in the fixed-width face on its ground, links, and mentions on the terracotta ground. The italic faces of the text font are carried in the binary. A link opens in the application of the system for its address. The window draws a link only for an address that the composer accepts: HTTP, HTTPS or mailto, without a user name. A server is not trusted with what this device opens (ADR 0714), so the check of the server (ADR 0408) is not relied on, and another address is drawn as text. The application of the system receives the address in its command line: on Windows the shell puts it in the place of `%1` in the command that the application registered, so a quote or a space in the address would end that argument and add others. The window therefore opens the address in one form only: the URL parser reads it, its serializer writes it with most such characters percent-encoded, and every byte that RFC 3986 does not allow in a URI is then percent-encoded too (the serializer keeps quotes and spaces in the part of a mailto address before `?`, and backslashes after it). The result holds no quote, space, control character, `<`, `>`, `\` or character outside ASCII, starts with its scheme, and is the same address; the tooltip of the link shows that form. Each system receives it as one value and builds no command line: on macOS as a URL object for the workspace, on Linux as one argument of the opening program or as the value of the desktop portal, on Windows as the file of the shell's `ShellExecute`.
- A code block is drawn on basalt with its language. A block that names a language gets syntax colours for five kinds of token: keywords, types, functions, values (strings, numbers, constants) and comments. The tokens come from the library `synoptic`, which works with regular expressions and carries its rules for about thirty languages. The window gives it no block above 32 KiB or above 1,000 lines, takes a panic of the library as "no tokens", and takes no token of a block that the library does not give back as it was written. A pass after the library removes a keyword, a type or a function name that lies inside a longer word, and marks a capitalised name as a type in the languages that write types so. The window holds the tokens of up to 256 blocks that it drew, by the rules of their language and their text. When a further block arrives, the block that was drawn longest ago is dropped. Reason: dropping all 256 made the window compute the tokens of every block in view again on the thread of the window. Test: `a_full_store_of_code_tokens_lets_go_of_the_block_drawn_longest_ago` in `crates/client`.
- A check box takes a click and sends an edit where the message can be edited: an own message that is not permanent (ADR 0402), in a channel that is not archived (ADR 0410). Elsewhere it is drawn without a click, because the server refuses the edit (ADR 0401).
- A file that the server marked as a picture (PNG, JPEG or WebP up to 1,024 pixels per side) is drawn inline at the size that its message states, so the row has its height before the picture has arrived. The picture is decoded away from the window with limits of 1,024 pixels per side. The window holds 48 decoded pictures, each for the owner that it was read for, and reads 4 at one time (ADR 0745). Every other file is a row with its name, its size and the state of its download.
- A download goes to the download directory of the person under a name that no file has, and the row then offers "Show in folder", behind the setting of ADR 0744 that also covers the opening of a file. While a download runs, the row offers its cancel, and the cancel removes the unfinished file; the test cancels a download before its first byte arrives. No file of a message is opened by the application. Test: `files::the_controls_of_a_file_download_it_cancel_the_download_and_show_the_file`.
- The window takes a change of the stage of a download or an upload of a file of a message, a change of its interruption, and each further hundredth of the file, as for the files of a recording (ADR 0744); a file without a stated length counts parts of 256 KiB. Reason: the window took the progress at each part that arrived or left, and each one derived the view again on the thread of the window. Test: `a_download_is_told_at_each_hundredth_and_at_its_end` in `crates/client`. The clippy rules of the crate refuse the progress watch of a download, an upload and a download of a recording everywhere except in the step reader (`state::progress::Steps`), so no reader of the window takes each part.
- A message whose files the retention of its workspace removed (ADR 0409) keeps its text, and its body states under the remaining files that its files were removed after the retention time of the workspace. The server marks such a message with `files_removed_by_retention`; without the sentence a message that had files would look as if it never had them. Test: `messages::a_message_whose_files_the_retention_removed_says_so`.

Actions of a message:

- React, reply, edit and delete are a bar at the message under the pointer. The same actions are a menu of the message: a right click, and from the keyboard the Menu key, Shift and F10, Enter or Space while the message holds the focus (ADR 0702). Each action is offered only where the server takes it; a message without an action has no menu, and no bar unless the bar states that the message is permanent.
- Edit is offered for an own message, and delete for an own message and with "manage messages". The roles are a prediction; the server decides (ADR 0739).
- A deletion asks first. A reaction, an edit and a deletion change nothing on screen before the server tells the change with its event; the answer to the request is not shown (ADR 0746). A refusal is told above the composer.
- The emoji of the picker are the table that the server uses, by group and by a search of their names.

Waiting messages:

- A message that the person sent is drawn at the end of the list at once, with its state: sending, uploading its files, no answer, or not sent with the reason. "Send again" and "Discard" are offered for the states that ADR 0721 names.

Composer:

- The composer is a text area that grows to 10 rows. Enter sends, and Shift with Enter adds a line.
- The text is typed with marks, and the sent message shows formatted text: `**bold**`, `*italic*`, `~~struck~~`, `` `code` ``, a block between lines of three backticks with an optional language, `> ` for a quote, `- ` and `1. ` for lists, `- [ ] ` for a check box, `[text](address)` and bare addresses for links, and a backslash before a mark for plain text. The toolbar of the design puts the marks around the selection.
- The work to read a text for its marks grows with its length and not with its square: the runs of backticks of a text are listed once, a kind of mark that has no closing run is not searched for again, the search for the end of a link stops at the longest address, and marks inside marks are read 8 levels deep. A test counts the steps. A text above 64 KiB is not read for marks and is refused for its length.
- A mention is a token of the text area and no mark: `@` opens a list of the members and roles of the workspace, and of "everyone" with its permission; the chosen entry is one unit of the text with the identifier of its account or role. In a direct conversation the server takes a mention of its two participants only (ADR 0408), so the list offers the other participant, once the client holds the name of that account. When a message is edited, the text of the composer writes each mention on one line: a control character of the name, such as a line break, is written as a space, so no prefix of a quoted line and no escape of a line that starts a block is put inside a mention, and its range stays on the bounds of its characters. Test: `compose::tests::a_name_with_a_line_break_is_written_as_one_mention_on_one_line`.
- An edit puts the text of the message into the composer with its marks. The arrow up in an empty composer edits the last own message. Escape ends an edit or a reply.
- The text of the composer cannot state every document that the server accepts: a line break in a list item, white space at the ends of a paragraph, an address in plain text that is no link. So an edit changes only what the person changed. A block at the start or at the end of the text that stands as the composer showed it is sent as the document had it, and so is an unchanged item of a changed list. A check box that the person changes in the text keeps the text of its item. The text between is read as the text of a new message.
- Files are attached with the file dialog, by a drop on the composer, and by pasting a picture. The files of a file dialog are attached only while the composer still writes to the place and for the account that the dialog was opened for (ADR 0746). An upload starts when the file is attached. The composer shows each file with its progress and a control that removes it.
- Before a send the document is checked against the limits of ADR 0408, and a file against the limits of the server. What would be refused is not sent, and the composer states the reason.
- The text, the reply and the edit of a place stay with that place and with the account that wrote them while the window shows another one. When a server has another account, the texts of the account before are discarded and the uploads of its files are ended.

Unread signs:

- A channel with unread messages has a bold name and a dot, and with mentions the sign "@ n" (view A). ADR 0741 makes both follow the notification settings of the person. A workspace of the server book and the icon of a collapsed server carry the same signs for their channels. 100 or more is shown as "99+".
- The read position moves to the last loaded message when the window is active, draws the list, and the list shows its end. The chat of a call is not drawn while the panel shows the members or the settings, and its messages stay unread until the chat tab is shown. "Mark as read" in the header moves the position to the latest one of the channel.

Places:

- A text channel and the chat of a voice channel are the same list and composer. The chat tab of a call (ADR 0735) shows the chat of its voice channel in the panel.
- The window holds the place of the main area open in the client core. ADR 0741 adds the thread of the side panel, with a list and a composer of its own, and direct conversations as places of the main area.

## Consequences

- The composer shows marks while the person types, not formatted text.
- The syntax colours follow regular expressions and no grammar: a name is coloured by its form, not by its meaning. A language that the library does not know, and a block without a language, stay without colours.
- A picture above the bounds of the server for pictures is a file row.
- The name of a channel in a text is plain text: the document of the server has no channel reference.
- The list draws the rows in view and a margin of 800 pixels, so the time of a frame does not grow with the loaded messages: measured with the window code and the renderer without a display in a release build on the development machine, a frame without a change and a frame after a scroll of 40 rows take 2.6 to 2.8 milliseconds with 50 and with 10,000 loaded messages (longest 4.8). A frame after a new message derives the rows again and takes 5.3 milliseconds with 50 messages and 9.1 milliseconds with 10,000 (longest 9.9). A debug build is about ten times slower. `junto-screens` repeats the measurement (`JUNTO_SCREENS_ONLY=frames`).
- A system line of an action that this build does not know is drawn as a line without detail.
- A finished recording is a card in the chat of its voice channel (ADR 0744).
- Direct conversations, threads and pins are in the window as ADR 0741 states.
- New direct dependencies of the desktop client: `chrono` for the time zone of the system, `dirs` for the download directory, `emojis` in the version of the server, and `url` for the check of a link. All four were in the lock file already.
