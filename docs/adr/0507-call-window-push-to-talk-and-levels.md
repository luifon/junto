# 0507. Call window, push-to-talk, levels and volume

## Status

Accepted

## Context

A user in a call works in other applications and on other servers during the call. The call needs controls that stay reachable, a way to speak only on a key press, and a display of who is speaking. A level meter needs a continuous stream of audio level values, which the client has only for the call it is in. Operating systems and display servers differ in what an application may do outside its own focused window.

## Decision

Push-to-talk:

- Push-to-talk works while a window of Junto is in front, and also while another application is in front where the system gives Junto the keys (ADR 0776).
- Voice activation is the default.
- A switch from voice activation to push to talk in the settings asks first. One click on "Push to talk" shows a question that names the key ("…only while you hold Option…") with "Use push to talk" and "Keep voice activation"; the mode changes only after "Use push to talk". The switch back to voice activation does not ask, because it cannot leave a person without sound. While push to talk is on, the call strip states it under the name of the call with the key: "Push to talk: hold Option to speak". Reason: one click changed the mode with no word, and a person who chose push to talk by accident sent no sound in a call and could not see why. Tests: `calls::the_sound_settings_of_the_application_are_kept_over_a_restart` reads that one click asks and does not change the mode, that "Keep voice activation" keeps it, and that "Use push to talk" changes it; `live::push_to_talk_and_a_block_of_a_moderator_in_the_window` reads the mark in the call strip.

Call window:

- The call can be detached into its own operating-system window. That window keeps the controls of the voice channel.
- The call window can be made full screen.
- People without video can be hidden in the call window (ADR 0735). They remain in the member list.
- Keeping the call window above other windows ("keep on top") depends on the system:
  - macOS: gpui sets the window level only when a window is created, and has no call that changes it later. The client switches "keep on top" by creating the call window again with the other window kind, or by a call to the window of the operating system through the raw window handle.
  - Linux on X11: the floating window kind of gpui keeps the call window above the main window only, and a window of another program covers it. The window manager keeps the window above other programs after the standard request for the window state "above", which gpui does not send. The client reads the identifier of the X11 window from the raw window handle and sends that request on a display connection of its own, because gpui does not give its connection. The request can be sent at any time; it changes the stack and leaves the focus where it is.
  - Linux on Wayland: not possible. The core Wayland protocols have no request by which a normal window stays above others, and on the one compositor that was tested the floating window was behind the main window after a click on the main window. "Keep on top" is not offered there.
  - Windows: not tested.

Levels and volume:

- For the voice channel that a user is in, the channel list shows a level meter per person.
- For other voice channels, the channel list shows only a mark for who is speaking. The clients in that voice channel report this to the server.
- A voice channel that hides who is inside shows neither.
- Each listener can set a volume per person.

## Consequences

- Push-to-talk and the key that mutes work while another application is in front on Windows, on X11, on macOS with the permission Input Monitoring, and on a Wayland desktop with the global shortcuts portal (ADR 0776).
- The speaking marks for other voice channels depend on reports from clients, so a modified client can send a false report.
- The server receives and distributes speaking reports for every voice channel that does not hide who is inside.
- A volume per person is a setting of the listener and does not change what other listeners hear.
- On Wayland the call window can be covered by other windows.
- The client has an interface for this request, which answers "applied", "unsupported" or "failed". It answers "unsupported" for a Wayland window and under an X11 window manager that does not list the state "above"; the choice "keep on top" is to be offered only under X11. For a window that the window manager has not taken yet, the state is also written as a property of the window, which the window manager reads when it takes the window. The detached call window is the caller of this interface (ADR 0772).
- The X11 request opens a connection to the display and waits for several answers of the display server. It runs on a thread of the client that sends the requests in the order in which they were asked for, and the answer is written to the log. Reason: the request ran on the thread of the window, which then waited for the display server at each opening of the call window and each change of the choice (ADR 0730). The queue is a type of its own, tested on every system: a caller returns while the work before it waits, and the work runs in its order (`queued_work_runs_in_its_order_and_its_caller_does_not_wait`). The X11 request itself runs only on Linux.
- The X11 request for the state "above" was observed with one window manager. A test runs it against the window manager Openbox on a virtual display: the window stays above a newer window, the focus does not change, the state can be taken away, and a window that is not shown yet gets the state when it is shown. The two ways to switch "keep on top" on macOS were read in the source of gpui and were not run (`docs/step-0/README.md`, test 2).
