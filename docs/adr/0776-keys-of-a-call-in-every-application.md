# 0776. The keys of a call in every application

## Status

Accepted. The key that mutes and the keys that the system reads are the key bindings of ADR 0777 since then; the readers below read every binding that may work outside.

## Context

Push to talk worked only while a window of Junto was in front (ADR 0507), and Junto had no key that mutes. Discord's push to talk and its mute key work while another application is in front. Each system gives the keys of other applications in its own way: Windows lets every program read the state of each key; an X11 server gives the keys that it holds down to every client; macOS gives the state of the keys of the hardware to a program that the person allowed under Input Monitoring; a Wayland session gives no program the keys of another one, and a desktop that implements the global shortcuts portal gives a program the presses of the shortcuts that it bound, after the person confirmed them.

## Decision

The keys:

- The key of push to talk is the key of the sound settings, Alt (Option on macOS) or Control (ADR 0507), until the person sets another binding for it. The key that mutes and unmutes is the binding "Mute or unmute" (Control+Shift+M), and every other binding of ADR 0777 that may work outside follows the same path; they are kept in the settings file of the interface (ADR 0777), not in the sound settings.
- A press of the key that mutes does what the button of the microphone does: it closes an open microphone of the call, and opens a closed one at the seat of the call (ADR 0788). Outside a call it does nothing. A microphone that the server holds closed stays closed.

The source of the keys, in the client core (`global_keys`):

- Windows reads the state of each key with `GetAsyncKeyState`. An X11 session reads the keys that the X server holds down with `QueryKeymap`, with the key codes of the layout of the server. macOS reads the state of the keys of the hardware with `CGEventSourceKeyState`, with the permission Input Monitoring: `CGPreflightListenEventAccess` reads it, `CGRequestListenEventAccess` asks for it, and without it the reader fails with `NotAllowed`.
- A reader is read every 15 milliseconds, on a thread of its own, only while a call needs a key: in a call, with push to talk on or a key that mutes set. Without a call nothing is read. Readings give events (`Edges`): the start and the end of a hold of the key of push to talk, and each press of the key that mutes, once while it is held. A reading that fails ends a hold, and the source states why.
- A Wayland session (a Wayland display and no X11 session) uses the global shortcuts portal of the desktop when a call first needs the keys: it makes a session and binds the shortcuts `push-to-talk` and `toggle-mute`, with the keys of Junto as the suggestion in the form of the desktop (`CTRL+SHIFT+m`). The desktop asks the person to confirm them, and can change them in its settings. Their presses and releases are the events; a press outside a call gives nothing, and the end of a call ends a hold. New keys make a new session. A desktop without the portal, a refusal or no answer within 120 seconds ends the source with its reason.
- A window of Junto that is in front gives the keys too: the key of push to talk through the modifier keys of the window (ADR 0507), and the key that mutes through its key presses while no source of the system gives it, so that a press is not counted twice. The calls send the microphone with push to talk while a window or the source holds the key.
- Only the start of the binary installs the source of the system. Every other start, such as a window test, has none, or the keys that a test holds (`HeldKeys`).

The settings:

- The section "Keybinds" of the settings (ADR 0777) states how the keys reach Junto on this system: also while another application is in front (Windows, X11, macOS with the permission); on macOS without the permission, how to give it, with "Allow", which asks macOS, and "Open System Settings", which opens the page of Input Monitoring (the permissions of the operating system, ADR 0708); with the portal, that the desktop asks to confirm them; on a desktop without the portal or after its failure, that they work only while a window of Junto is in front. The section "Voice" points to it.

Tests: `global_keys::tests` (the combinations, the edges, the reader loop with a failing reader), `global_keys::portal::tests` (a portal of a desktop on a private session bus: the binding when a call needs the keys, the presses and releases, a press outside a call, a refusal, a desktop without the portal), the window tests `calls::the_keys_of_a_call_reach_the_call_from_the_source_and_from_the_window`, the X11 reader against the X server of the run with keys that `xdotool` presses (`global_keys::x11::tests`, step "Keys of a call read from the X server" of the Linux CI job), and the Windows reader with F24 pressed through `SendInput` (`global_keys::windows::tests`, step "Keys of a call read from the keyboard" of the Windows CI job). The reader of macOS and the permission are checked by a person on a Mac.

## Consequences

- The key of push to talk and the key that mutes work while another application is in front on Windows, on X11, on macOS after the person allows Input Monitoring, and on a Wayland desktop with the portal after the person confirms the shortcuts.
- macOS lists Junto under Input Monitoring once it asked. A build that is signed with another identity asks again (ADR 0725).
- On Wayland the desktop decides the keys: the settings of Junto suggest them, and the settings of the desktop show the ones in force.
- While push to talk is on, holding Option or Alt in another application also sends the microphone, as in Discord.
- The reader of the keyboard runs only in a call, so a client outside a call reads no key.
