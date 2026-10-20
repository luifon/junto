# 0772. The detached call window

## Status

Accepted

On Linux a floating window kind does not keep a window above the windows of other programs. The request of ADR 0507 to the window manager does, under X11; a Wayland window has no such request.

## Context

ADR 0507 allows a call in a separate window while the main window keeps conversations available. Both windows operate the same call, and ADR 0710 requires installation settings to survive a restart.

## Decision

- The detached window tells what the main window tells about the call: the notices of the call (an end of a block, a refused request, an action that had no effect, an invitation, a request to enter), the bar of meeting mode with its controls, the line about a microphone or a sound that cannot be used, and a share or a recording that failed. A person can use the call window alone.
- The detached window has the panel of the call with Chat, Members and Settings, drawn by the functions of the main window (ADR 0735): the chat of a voice channel or the conversation of a direct-message call with its composer, the people with their volume and the invitation of members, and the devices, the settings of the share and of the view and the settings of the room. The panel starts closed. Three buttons at the end of the first line of the header open it at one tab, and a button of the panel closes it. The panel lies beside the stage when the window is at least as wide as the panel and 480 pixels of the design, and over the stage otherwise. The notice area stays under the stage. The window and recording controls of the header wrap among themselves, so that the buttons of the panel keep their place when the recording control changes.
- The call window draws the place of the call whatever the main window shows. For a direct-message call it selects the conversation of the two accounts; until the state of the server holds that conversation, which the first ring of the call creates (ADR 0518), the window draws the stage of the call without a panel.
- Each window has its own composer, as each has its own list of messages. When the call window closes, its composer gives the text of each place that it holds to the main window: a place whose composer is empty there takes the text with its reply or its edit, and a place that holds text there gets the other text after it on a new line. The call window that opens again for the choice of keeping it on top takes that text in place of the main window.
- The detached window shares the application state and media connection with the main window. Detaching never starts another call or capture. The main window shows that the call is separate and offers Return.
- Return and closing the detached window put the call back in the main window. Closing the main window closes its detached call window as part of application shutdown. Leave ends the call. The detached window closes when the call ends; a temporary connection loss shows reconnecting in that window. Token renewal preserves the window, and a moderator move follows the call to its destination. The title follows the call and includes the server name.
- `call-window.json` stores the display UUID, windowed position, size and always-on-top choice in the installation data directory. Its versioned format rejects invalid dimensions and preserves a format newer than this client. Writes use the data-directory lock and atomic replacement. Consecutive bounds changes replace one pending placement; one background writer drains the final placement before application shutdown.
- Restored bounds use the saved display UUID and are clamped to that display. A missing display centres the window on an available display. Fullscreen bounds do not replace the stored windowed bounds.
- Always-on-top changes recreate the window with the selected kind while keeping the call. The macOS floating kind requests the elevated window level; native deactivation and fullscreen behavior require platform verification. On Linux the window keeps the normal kind and is not created again: the call window sends the request of ADR 0507 (`window_level::keep_above`) when it opens with the choice and when the person changes it, and the choice is drawn only where `window_level::offered` holds, which is under X11. A request that the system does not take is written to the log, and the call goes on.
- Push-to-talk belongs to focused windows. Losing focus or closing a window releases that window's contribution; no background window keeps a microphone open through a stale key state.

## Consequences

The call continues when either window changes its selected conversation. Tests on the test platform verify ownership, controls and stored bounds; they do not verify native fullscreen or the ordering of real windows.

While the call is detached, both windows show the chat of the call, each with its own composer, and a list scrolled in one window sets the place in the timeline from which the client reads its pages (ADR 0721) for both. The chat of a call has no threads in either window.

Tests: `live::detached::the_call_window_has_the_chat_the_members_and_the_settings_of_its_call` opens the panel in the call window, reads and writes the chat, sets the volume of a person, chooses a microphone, closes the panel, and finds the text of the composer of the call window in the main window after Return. `live::detached::the_call_window_of_a_direct_call_has_its_conversation` reads and writes the conversation of a direct-message call in the call window.
