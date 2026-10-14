# 0772. The detached call window

## Status

Accepted

Open: Linux needs a platform hook for the stored always-on-top choice. A floating window kind alone does not establish that behavior on every window system.

## Context

ADR 0507 allows a call in a separate window while the main window keeps conversations available. Both windows operate the same call, and ADR 0710 requires installation settings to survive a restart.

## Decision

- The detached window tells what the main window tells about the call: the notices of the call (an end of a block, a refused request, an action that had no effect, an invitation, a request to enter), the bar of meeting mode with its controls, the line about a microphone or a sound that cannot be used, and a share or a recording that failed. A person can use the call window alone.
- The detached window shares the application state and media connection with the main window. Detaching never starts another call or capture. The main window shows that the call is separate and offers Return.
- Return and closing the detached window put the call back in the main window. Closing the main window closes its detached call window as part of application shutdown. Leave ends the call. The detached window closes when the call ends; a temporary connection loss shows reconnecting in that window. Token renewal preserves the window, and a moderator move follows the call to its destination. The title follows the call and includes the server name.
- `call-window.json` stores the display UUID, windowed position, size and always-on-top choice in the installation data directory. Its versioned format rejects invalid dimensions and preserves a format newer than this client. Writes use the data-directory lock and atomic replacement. Consecutive bounds changes replace one pending placement; one background writer drains the final placement before application shutdown.
- Restored bounds use the saved display UUID and are clamped to that display. A missing display centres the window on an available display. Fullscreen bounds do not replace the stored windowed bounds.
- Always-on-top changes recreate the window with the selected kind while keeping the call. The macOS floating kind requests the elevated window level; native deactivation and fullscreen behavior require platform verification. Other platforms must apply an equivalent window-level request or report that it is unavailable.
- Push-to-talk belongs to focused windows. Losing focus or closing a window releases that window's contribution; no background window keeps a microphone open through a stale key state.

## Consequences

The call continues when either window changes its selected conversation. Tests on the test platform verify ownership, controls and stored bounds; they do not verify native fullscreen or the ordering of real windows.
