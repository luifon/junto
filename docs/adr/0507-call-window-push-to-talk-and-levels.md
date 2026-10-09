# 0507. Call window, push-to-talk, levels and volume

## Status

Accepted

## Context

A user in a call works in other applications and on other servers during the call. The call needs controls that stay reachable, a way to speak only on a key press, and a display of who is speaking. A level meter needs a continuous stream of audio level values, which the client has only for the call it is in. Operating systems and display servers differ in what an application may do outside its own focused window.

## Decision

Push-to-talk:

- Push-to-talk works while the application or the detached call window is focused.
- Voice activation is the default.

Call window:

- The call can be detached into its own operating-system window. That window keeps the controls of the voice channel.
- The call window can be made full screen.
- People without video can be hidden in the call window. They remain in the member list.
- Keeping the call window above other windows ("keep on top") depends on the system:
  - macOS: gpui sets the window level only when a window is created, and has no call that changes it later. The client switches "keep on top" by creating the call window again with the other window kind, or by a call to the window of the operating system through the raw window handle.
  - Linux on X11: the floating window kind of gpui keeps the call window above the main window only, and a window of another program covers it. The window manager keeps the window above other programs after the standard request for the window state "above", which gpui does not send. The client sends that request through the raw window handle. The request can be sent at any time.
  - Linux on Wayland: not possible. The core Wayland protocols have no request by which a normal window stays above others, and on the one compositor that was tested the floating window was behind the main window after a click on the main window. "Keep on top" is not offered there.
  - Windows: not tested.

Levels and volume:

- For the voice channel that a user is in, the channel list shows a level meter per person.
- For other voice channels, the channel list shows only a mark for who is speaking. The clients in that voice channel report this to the server.
- A voice channel that hides who is inside shows neither.
- Each listener can set a volume per person.

## Consequences

- Push-to-talk does not work while another application is focused. System-wide push-to-talk is in the later list of `docs/scope.md`.
- The speaking marks for other voice channels depend on reports from clients, so a modified client can send a false report.
- The server receives and distributes speaking reports for every voice channel that does not hide who is inside.
- A volume per person is a setting of the listener and does not change what other listeners hear.
- On Wayland the call window can be covered by other windows.
- The X11 request for the state "above" was observed with one window manager. The two ways to switch "keep on top" on macOS were read in the source of gpui and were not run (`docs/step-0/README.md`, test 2).
