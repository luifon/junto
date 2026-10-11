# 0735. The screens of a voice call

## Status

Accepted

## Context

The approved design draws a call with a screen share (view B), its problem states (view F) and the chat of a voice channel that the person is not in (view G). It draws no call without a share. This version has the voice of a call and no camera, no screen share and no text messages (ADR 0903). ADR 0730 lets the window draw no control of a function that the build lacks, with the exception of the profile strip. The call can later be moved into a window of its own (ADR 0507).

## Decision

Entering:

- A click on a voice channel shows the channel and joins nothing. Its header has "Join call" (view G). The same place shows the state of its call: recording, meeting mode, lock, count. For the call of this device the header has the controls of the room in their place (ADR 0739).
- A join that the server refuses is told by a notice above the content, with the name of the channel and what the person can do. The notice stays until the person closes it.

The call in the main area:

- The header has the title and the state of the call. While the call is restored, it shows "Reconnecting…" and the statement that the room controls return (view F).
- The stage shows the people as tiles: the generated avatar, and a plate with the name and the level meter or the mark of the microphone. The tile of a person who speaks has the bright edge. A tile is one and a half times the tile of the strip of the design.
- While the first media connection is made, and while the call is restored, the stage shows that statement in place of the tiles.
- The panel has the tabs Chat, Members and Settings. Chat states that messages are not in this version. Members lists the people in the call with their state and the volume control of each other person, and the members of the workspace outside the call with the voice channel that they are in. Settings has the input device with the input level, the output device, and the state of the microphone permission.
- In a narrow window the panel starts closed, three buttons of the header open its tabs, and it lies over the stage (ADR 0731).

Outside the main area:

- The channel list shows, for the call of the device, the measured level of each person (ADR 0507), and "connecting…" or "reconnecting…" beside the person of the device.
- The strip of the call is above the profile strip while the device is in a call, whatever server and channel are shown. It names the voice channel, leads to it, shows the time since the call started, and has "Leave". Its share button starts and ends a share (ADR 0738), and its hand button raises and lowers the hand of the person (ADR 0739). Its camera button is drawn disabled, so that the strip has its final form.
- The microphone and the sound button of the profile strip work in a call and outside one. A microphone that the person closed has the crossed mark on terracotta; a microphone that the server holds closed has the lock mark and takes no click.
- A line above the profile strip tells who closed the microphone or the sound, and that the microphone cannot be used, with "Try again".
- The settings of the application have the part "Voice": the input mode, the key of push-to-talk, and the three parts of the processing.

Structure:

- The functions that draw the call take the derived view (ADR 0730) and the handlers of the window, and read from the main window only what belongs to the drawing: the open tab, the open list of devices, the positions of the volume controls, the scroll positions of the lists, and whether the panel is open in a narrow window. A second window draws the same view with them.

## Consequences

- A person sees the people of a call that has no share. The design has no drawing of that state; it was built from the tile, the plate and the meter of the design.
- The devices are chosen in the call, as the design places them. A person who wants another device before a call chooses it in the first call; the choice is stored.
- The volume control is the unstyled slider of the component library with the look of the design. The arrow keys change the volume by 10 %.
- The list of devices is drawn at its box and above the panel. It holds the keyboard focus while it is open.
- When a control that holds the keyboard focus leaves the window, such as "Join call", the focus returns to the window, so that Tab and the key of push-to-talk still reach it.
- The window draws one disabled button of a function that this version lacks, the camera, as an exception to ADR 0730.
- The time of a call is computed from the time of the server and the clock of the machine. A machine whose clock is behind shows 00:00:00 at first.
