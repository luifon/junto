# 0781. The account layer: profile and devices

## Status

Accepted

## Context

An account has a name and an avatar per server (ADR 0119) and one or more devices, each with its own key (ADR 0104, ADR 0112). The client core has the requests for all of it and the flow that approves a new device (ADR 0712). The window showed none of it: the settings hold the profile preset, which is the start of a new account and not the profile of an account that exists (ADR 0717). The administration layer (ADR 0761) fixed how a layer with pages behaves: one request of the person at a time, a refusal at the control that asked, and no answer that reaches another page, another server or another account.

## Decision

One layer:

- "Your account" is an item of the menu of the selected server. It opens one layer above the window, in the frame of the administration layer: a list of pages at the left and one page at the right. The pages are "Profile", "Devices" and "Recoveries".
- The layer follows the rules of ADR 0761 for requests. Each request has the states sending and refused. A refusal is a sentence at the control that asked. Only the answer to the newest request of the open page changes what the page tells; an answer that arrives after the page changed or the layer closed is dropped.
- A page sends one request at a time. While a request is on its way, the state of the layer starts no other request of that page, and the controls of the page that send one are disabled. Reason: the page holds the state of one request, so a second request would hide the first one and its refusal. What the page tells about a request that is on its way is not put away.
- Every request of the layer is sent for the account that the layer was opened for (ADR 0714). When the server of the layer leaves the server book, or shows another account or none, the layer closes and keeps nothing.
- A form and a question are drawn in the row that they belong to. No second layer opens above the first. When the answer closes a form or a question of a device, its row keeps the height that it had with it, as ADR 0730 states.

Profile:

- The page shows the name and the picture that the server holds. Both come from the state of the server, so a change is shown when the server has recorded it, whichever device made it.
- The name field starts with the name of the server. "Save" sends it. The name is checked by the rule of the client core before a request; a name that the rule refuses is told at the field and not sent. The button reads "Saved" while the field holds the name of the server.
- "Choose a picture…" opens the file dialog of the system. The page states the bounds of ADR 0119 under the button.
- The chosen file is read on a background thread, at most one byte beyond 10 MiB, decoded, and cut in the dialog of ADR 0119; the cut is checked again (at most 2 MiB, its first bytes) before the request. Only a regular file is read, by the function of ADR 0783: a link is not followed, and a pipe or a device is refused without a wait. Its size and its first bytes are checked before a request. A file that is too large, or that is no PNG, JPEG or WebP image, is told at the button and not sent. The server checks the size in pixels; its refusals `image_too_large` and `image_unsupported` are told at the same place.
- A file that the dialog returns after the layer closed, or for another server or account than the one it was chosen for, is not sent.
- A file that the dialog returns while another request of the page is on its way is not sent. The page says so at the button, and the person chooses the file again.
- "Remove picture" removes the avatar. It is disabled while the account has none.
- The limit of 30 changes per minute (ADR 0119) is told with the time of the `Retry-After` header.

Devices:

- The list is read from the server when the page opens. It is read again after a change that the page made, when the server tells `device_added`, `device_renamed` or `device_removed`, when it tells `device_logged_in`, which changes the last login of a device, and when the state of the server was replaced or the reader of its events fell behind.
- One read runs at a time. Changes that arrive during a read cause one further read after it. A read that failed is not repeated by itself: the page states the failure with "Try again", and keeps the rows of the read before.
- The layer holds at most 200 devices. When the server lists more, the page states how many it does not show.
- A row shows the name, or "Unnamed device"; "This device" for the device of the session; how the key came to the account; the date of the addition; and the last login. Times are in UTC, as the notice of a recovery (ADR 0732).
- "Rename" shows a field in the row. A device name can be empty (ADR 0112).
- "Remove" asks first. The question is one of two fixed sentences and holds no name; the name of the device stays on its line above the question. The question for the device of the session states that its session ends and that this installation loses its access. The removal of the only device is not offered: the button is disabled and the reason is a sentence under the list. The server refuses that removal too (ADR 0112), and its refusal is told at the row.
- When the person removes the device of the session, the server ends the session, the server shows the state "removed", and the layer closes because its account is gone.

Recoveries:

- The page lists every recovery of the account (`GET /api/v1/account/recoveries`, ADR 0112), the newest first, also those that this device has acknowledged (ADR 0118). Reason: the notice of a recovery goes when the person states that it was seen, and the person can still need to know later that the account was recovered and when.
- A row states the time in UTC and whether the server owner or the host command of the server did it. It states "It added this device" when the recovery added the device of the session, and "Not seen on this device yet" while this device has not acknowledged its notice. The page acknowledges nothing: a notice is acknowledged only by its button (ADR 0730).
- The list is read when the page opens, again when the server tells `account_recovered`, and when the state of the server was replaced or the reader of its events fell behind. One read runs at a time, as for the devices. A read that failed is stated with "Try again" and keeps the rows of the read before.
- The layer holds at most 200 recoveries, the newest ones, and states how many older ones it does not show.
- Test: `recovery::the_owner_recovers_an_account_onto_a_new_device_and_the_account_lists_the_recovery` reads the page on the recovered device: one recovery by the owner that added this device and is not seen, and after "I have seen this" the same recovery as seen. The test failed before the layer had the page.

Approval of a new device:

- "Link a new device" on the devices page is the approving side of ADR 0112. It draws the four states of the flow of the client core: the entry of the code, the key to compare with "Approve" and "Cancel", "Device linked", and "outcome unknown" with "Check again".
- The key of the new device is shown as the text that the command-line tool prints for a key, in groups of four characters (ADR 0782).
- The flow belongs to the open page. It holds the code from the start of the lookup. The code field is emptied when the lookup starts, when the person cancels, when the link ends, and when the layer closes. Reason: the field needs the code only until the flow has it, and a code is not kept longer than it is needed. After a refusal, and after a lookup without an answer, the person types the code again.
- An approval reads the list of devices again also when the event of the server was missed.

Text of a server:

- A name of an account, of a device or of a server is shown without control characters, cut at 64 characters, in a line of its own that is cut at the width of its place.
- A sentence that asks a question or tells the person what to do is fixed text and holds no name of a server. A statement can hold a name in quotation marks.

## Consequences

- A person changes the name and the picture of one account without a change of the profile preset or of the accounts on other servers.
- The picture on this page is the avatar component of ADR 0783: the image of the avatar when the window holds it, and the generated avatar of the account otherwise.
- A file larger than 10 MiB costs one read of 10 MiB and one byte, and no request.
- A server that tells device events without end causes one read after another, never two at once, and only while the devices page is open.
- A person who removes the device of the session by mistake needs a device link or a recovery to return.
- The name field does not follow a change of the name that another device makes while the page is open; the button then reads "Save" again.
- The typed link code is held by the text input of the component library until the lookup starts, in memory that this crate cannot overwrite (ADR 0732 states the same of an invite code).
- A person whose lookup got no answer types the 8 symbols of the code again.
- A person who saves the name and chooses a picture does the second after the first has ended.
