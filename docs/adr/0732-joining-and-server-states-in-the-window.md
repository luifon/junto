# 0732. Joining a server and the states of a server in the window

## Status

Accepted

## Context

ADR 0105 defines joining with an invite code and with a request that an administrator approves, and ADR 0712 gives the flows that carry it. The approved design types the address in the server book and shows one dialog with the server that answered, a notice about the operator, and two ways in. A person on a new server has the owner code that the server printed, not an invite code (ADR 0110, ADR 0114). The window must also show what a saved server is when it is not usable: unreachable, waiting, banned, removed, incompatible, or answering as another server (ADR 0712, ADR 0713, ADR 0714).

## Decision

First start:

- The application asks for a name before anything else when the profile preset has none. The name is stored as the preset (ADR 0717) and is the name of each account that this device creates. The dialog has no close control.

Joining:

- The address field of the server book starts the flow "add a server". A failure is told under the field. When the server answered, the dialog "Join a server" opens with its name and address.
- The dialog states that the person who operates the server has technical access to its data.
- One code field takes an invite code and an owner code. A text with 20 symbols is sent as an owner code and every other text as an invite code (ADR 0110 fixes both lengths). An empty field is not sent.
- "Request access" sends a join request with the name of the preset.
- The server is saved to the server book when the person chooses a way in, with the connection mode "always" (ADR 0107). Before that nothing is stored and no key exists.
- A way in that the server refuses leaves the server in the book without an account. The dialog stays open with the reason, and the main area of that server offers the dialog again.
- After a request the dialog says that the request was sent and under which name, and can be closed. It names no time: the client core stores none.
- "Cancel request" forgets the request (ADR 0712) and removes the server from the book.
- The dialog closes by itself when the device has its account. The code field is emptied when the dialog closes.
- The result of an action is shown in the dialog that started it. When that dialog was closed before the action ended, the result is in the state of the server and changes no dialog that was opened later.
- When an administrator decides a request while the person cancels it, nothing is removed: the state of the server shows the decision.
- An action that the person asks for waits for an action that the client runs by itself on the same server, such as the check of a waiting request, and is not refused as `busy` (ADR 0712).
- A server without a common API version can be added to the book from the dialog and not joined (ADR 0713). A key of the server for another address is told with the action that discards it (ADR 0711).

States of a server:

- The server book shows under the name of a server its address, or one line for its state: connecting, reconnecting, not logged in, waiting for approval, waiting for a device link, not joined, removed, request denied, request expired, link expired, key missing, banned, cannot be reached, needs a newer client, server too old, not compatible, another server answers, recovery notice.
- A selected server without lists shows its state in the main area with the actions that the state accepts: check again, join, cancel the request, forget the account, remove the server.
- A server whose connection is interrupted keeps its lists, and a line above the profile strip says that they are the last known state (ADR 0719).
- A server that this device has an account on and that cannot be reached is contacted again every 20 seconds (ADR 0730).
- A recovery that the device has not acknowledged is a notice above the content of its server, with the time in UTC and the button "I have seen this" (ADR 0118).
- The menu of the selected server sets its connection mode and removes the server from this device. Removing and forgetting ask before they act, because neither can be undone on the device (ADR 0712). The question has a fixed title, and the name of the server, which the server chose, has a line of its own that is cut at the width of the dialog.

## Consequences

- A person joins a new server of their own with the owner code in the same field as an invite code.
- A code with 20 symbols that is not an owner code is sent to the claim route and refused there with `owner_code_invalid` or `owner_already_claimed`. The sentence for the second names the invite code.
- A server whose join failed stays in the server book until the person removes it.
- A request that was sent in an earlier run is shown as waiting without the time of the request.
- A ban, a removal and an unban are shown without an action of the person when the event stream tells them, and at the next check otherwise.
- The time of a recovery is shown in UTC until the client reads the time zone of the system.
- A typed code is held by the text input of the component library, in memory that this crate cannot overwrite (ADR 0714 states the same of the HTTP library). A code is used once.
