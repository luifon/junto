# 0772. The detached call window

## Status

Accepted

On Linux a floating window kind does not keep a window above the windows of other programs. The request of ADR 0507 to the window manager does, under X11; a Wayland window has no such request.

## Context

ADR 0507 allows a call in a separate window while the main window keeps conversations available. Both windows operate the same call, and ADR 0710 requires installation settings to survive a restart.

## Decision

- The detached window shares the application state and media connection with the main window. Detaching never starts another call or capture. The main window shows that the call is separate and offers Return.
- Return and closing the detached window put the call back in the main window. Closing the main window closes its detached call window as part of application shutdown. Leave ends the call. The detached window closes when the call ends; a temporary connection loss shows reconnecting in that window. Token renewal preserves the window, and a moderator move follows the call to its destination. The title follows the call and includes the server name.
- `call-window.json` stores the display UUID, windowed position, size and always-on-top choice in the installation data directory. Its versioned format rejects invalid dimensions and preserves a format newer than this client. Writes use the data-directory lock and atomic replacement. Consecutive bounds changes replace one pending placement; one background writer drains the final placement before application shutdown.
- Restored bounds use the saved display UUID and are clamped to that display. A missing display centres the window on an available display. Fullscreen bounds do not replace the stored windowed bounds.
- Always-on-top changes recreate the window with the selected kind while keeping the call. The macOS floating kind requests the elevated window level; native deactivation and fullscreen behavior require platform verification. On Linux the window keeps the normal kind and is not created again: the call window sends the request of ADR 0507 (`window_level::keep_above`) when it opens with the choice and when the person changes it, and the choice is drawn only where `window_level::offered` holds, which is under X11. A request that the system does not take is written to the log, and the call goes on.
- Push-to-talk belongs to focused windows. Losing focus or closing a window releases that window's contribution; no background window keeps a microphone open through a stale key state.

## Consequences

The call continues when either window changes its selected conversation. Tests on the test platform verify ownership, controls and stored bounds; they do not verify native fullscreen or the ordering of real windows.
