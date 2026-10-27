# 0735. The screens of a voice call

## Status

Accepted

## Context

This version has the voice of a call and no camera, no screen share and no text messages (ADR 0903). ADR 0730 lets the window draw no control of a function that the build lacks, with the exception of the profile strip. The call can later be moved into a window of its own (ADR 0507).

## Decision

Entering:

- A click on a voice channel shows the channel and joins nothing. Its header has "Join call". The same place shows the state of its call: recording, meeting mode, lock, count. For the call of this device the header has the controls of the room in their place (ADR 0739).
- A join that the server refuses is told by a notice in the notice area of the shown view, with the name of the channel and what the person can do. The notice stays until the person closes it.

The call in the main area:

- The header has the title and the state of the call. While the call is restored, it shows "Reconnecting…" and the statement that the room controls return.
- The stage shows the people as tiles: the generated avatar, and a plate with the name and the level meter or the mark of the microphone. The tile of a person who speaks has the bright edge. Tile sizes are defined in `crates/client/src/ui/tile.rs`.
- The tiles of the stage fill it, as in the grids of Discord and Meet (`ui::tile::grid`): each tile has the largest 16:9 size with which all tiles fit in the free area of the stage, with 8 design pixels between them. A wide stage has no more rows than columns and a tall stage no more columns than rows, so one person is drawn at the width of the stage, two people side by side at about half of it each, and four and nine people in two and three rows. Below the size of a tile of the strip (148 by 84) the tiles keep that size and the stage scrolls. The stage measures its free area when it is drawn and draws the tiles again at a new size, so a resize or a statement under the tiles changes the size of the tiles in the next frame. People that "Hide people without video" leaves out are not counted, so the others grow. In the focus the focused tile has the largest size above the rows of tiles of the size of the strip, which hold the other people: the strip wraps into as many rows as its tiles need across the stage, and the focused tile leaves room for each of them (`ui::tile::strip_height`; open item RP4: the focused tile left room for one row while the strip wrapped into more, and the rows under it ended outside the stage). Test: `ui::tile::tests::the_strip_reserves_each_row_that_its_tiles_wrap_into`. The strip under a share keeps its own size (ADR 0738). Reason: the tiles had a fixed size of 222 by 126, so two people were two small tiles in the middle of a large empty stage. Tests: `ui::tile::tests` (1, 2, 4 and 9 tiles in a stage of 1176 by 746 and of 656 by 380, and many tiles), and `live::cameras::the_focus_draws_the_camera_of_the_person_who_speaks_large_and_the_grid_returns`, which reads that two tiles are each between 40% and 50% of the width of the stage.
- While the first media connection is made, and while the call is restored, the stage shows that statement in place of the tiles.
- "Hide people without video" (Settings, "Your view") leaves out the tile of every person without a camera in each layout of the stage: the grid, the focus, the strip under a share (ADR 0738) and the call window (ADR 0772). A person has video while a camera of that person is published in the call; the camera of this device counts while it starts or runs. A camera counts before its first picture arrives, because only a drawn tile asks the media server for its picture (ADR 0770): a rule by the picture never drew a camera that started while the setting was on. The setting also leaves out every tile when nobody has a camera, and the stage then states it. A statement with the avatars of the first three people left out says how many the stage leaves out ("2 people without video are hidden") and that they are listed in Members. Reason: the setting first applied only while at least one picture was drawn, so in a call without cameras it changed nothing, and a person saw no effect. Test: `live::cameras::the_people_without_video_are_hidden_in_every_layout` reads the statement and the avatars of the people left out in the grid, the focus, the strip and the call window.
- The panel has the tabs Chat, Members and Settings. Chat shows the chat of the voice channel with its composer (ADR 0740). Members lists the people in the call with their state and the volume control of each other person, and the members of the workspace outside the call with the voice channel that they are in. Settings has the input device with the input level, the output device, and the state of the microphone permission.
- In a narrow window the panel starts closed, three buttons of the header open its tabs, and it lies over the stage (ADR 0731).

A direct-message call:

- A direct conversation whose direct-message call this device is in is drawn as the call of a voice channel: the stage with the people, the notice area at the end of the stage, and the panel with Chat, Members and Settings. Chat is the conversation with its composer. Members lists the people in the call with the volume control of the other person. Settings has the camera, the input and output devices, the settings of the own share and of the view, as in a voice channel. A direct-message call has no room (ADR 0501): the header, the panel and the call window have no room controls, no recording, no meeting mode and no invitation.
- The header has the name of the other person and the state of the call: "Calling…" until the other person has a place in the call, then "Direct call". While the call rings, the stage states that it calls the person, and Members lists the person outside the call.
- A share is drawn as in a voice channel: the bar of the shares when both people share, in which a card chooses the watched share, the line of the watched share with its sound, and the volume of the watched share and the quality of the own share in Settings (ADR 0738).
- Outside its call, a direct conversation has the button that starts a call above its messages, and the outcome of the last call (ADR 0771). When the device enters the call of the conversation or leaves it, and when the call moves into the call window or back (ADR 0772), the main area changes from one body to the other. That is a change of the whole body: its controls can change position (ADR 0730), as at the join of a voice channel.

Outside the main area:

- The channel list shows, for the call of the device, the measured level of each person (ADR 0507), and "connecting…" or "reconnecting…" beside the person of the device.
- The panel of the call (Chat, Members, Settings) closes with the button at the end of its tabs, in the main window and in the separate call window, as the server book collapses; the stage then takes its width. The buttons of the three tabs in the header open it again, and the Chat button counts the messages of the chat that arrived while it was closed. Whether the panel beside the stage is closed is kept in `settings.json` for each of the two windows and used after a restart. In a window too narrow for the panel beside the stage, the panel lies over the stage while it is open, as before, and that state is not kept. `Shell::toggle_call_panel` closes or opens it; the shortcut list (branch D) binds a key to it. Test: `panel_widths::the_panel_of_a_call_closes_and_opens_again_and_stays_closed_after_a_restart`.
- The strip of the call is above the profile strip while the device is in a call, whatever server and channel are shown. It names the voice channel, leads to it, shows the time since the call started, and has "Leave". Its share button starts and ends a share (ADR 0738), and its hand button raises and lowers the hand of the person (ADR 0739). Its camera button is drawn disabled, so that the strip has its final form.
- The microphone and the sound button of the profile strip work in a call and outside one. A microphone that the person closed has the crossed mark on terracotta; a microphone that the server holds closed has the lock mark and takes no click.
- A line above the profile strip tells who closed the microphone or the sound, and that the microphone cannot be used, with "Try again".
- The settings of the application have the part "Voice": the input mode, the key of push-to-talk, and the three parts of the processing.

Structure:

- The functions that draw the call take the derived view (ADR 0730) and the handlers of the window, and read from the main window only what belongs to the drawing: the open tab, the open list of devices, the positions of the volume controls, the scroll positions of the lists, and whether the panel is open in a narrow window. A second window draws the same view with them.
- The derived view has one panel of a call for both places: the people of the call, its media and settings, and its place. The place is a voice channel, with its name, the members outside the call, the recording and the room, or a direct-message call, with the other account and whether that account has a place in the call. The functions that draw the stage, the shares and the panel read the people from the panel, so they draw both kinds of call.

The notice area is the fixed scrolling area at the end of a call stage, the area above a message list and under its direct-call controls, or the part below the actions of a state panel (ADR 0730).

## Consequences

- A person sees the people of a call that has no share. The state uses the same tiles, name plates and meters as a call with a share.
- The devices are chosen in the call. A person who wants another device before a call chooses it in the first call; the choice is stored.
- The volume control is the unstyled slider of the component library with the theme of ADR 0731. The arrow keys change the volume by 10 %.
- The list of devices is drawn at its box and above the panel. It holds the keyboard focus while it is open.
- When a control that holds the keyboard focus leaves the window, such as "Join call", the focus returns to the window, so that Tab and the key of push-to-talk still reach it.
- The window draws one disabled button of a function that this version lacks, the camera, as an exception to ADR 0730.
- The time of a call is computed from the time of the server and the clock of the machine. A machine whose clock is behind shows 00:00:00 at first.
- During a direct-message call the conversation is read in the panel of the call, which is narrower than the main area. A person who wants the conversation in the whole main area detaches the call (ADR 0772) or leaves it.

Tests: `live::direct::a_direct_call_has_the_chat_the_volume_the_devices_and_the_share_controls_of_a_call` in the window live tier of `crates/client` starts a direct-message call in the window, reads the ringing call, sets the volume of the other person, reads and writes the conversation in the panel, chooses the microphone, the speaker and the camera, shares a screen with its sound, watches the share of the other person, sets the volume of the watched share and the quality of the own share.
