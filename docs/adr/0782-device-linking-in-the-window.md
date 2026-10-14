# 0782. Device linking in the window

## Status

Accepted

## Context

A second device gets access to an account through a link that a logged-in device approves (ADR 0104, ADR 0112). One flow links a new device on every server of the first device, with two texts that the person carries (ADR 0716). The client core has the flows of both sides for one server and for all servers (ADR 0712), and the command-line tool runs them. The window could show that a server waits for a link and nothing else: it could not ask for a link, it did not show the key of the device, and it had no view for the link on all servers. The approval names a public key (ADR 0112), so both devices must show that key in one form that a person can compare.

## Decision

The key that a person compares:

- For a link on one server both devices show the public key of the new device. Its text is the text that the command-line tool prints for a key, unpadded base64url with 43 characters, written in groups of four characters. One function of the text catalog makes it, and both sides use it.
- Reason: a device with the command-line tool and a device with the window can be compared, and the text is the whole key and not a shortened form of it.
- For a link on all servers both devices show the fingerprint of ADR 0716.

One server, the new device:

- The dialog "Join a server" has a third way: "I already have an account: link this device". It runs the action "request link" of the access flow with the device name of the application (ADR 0730). The server is saved when the person chooses this way, as with the other two (ADR 0732).
- While the device waits, the dialog and the main area of the server show the link code in large fixed-width type, the key of this device, the time at which the code expires in UTC, and what the person does on the logged-in device.
- "New code" sends a new request, which replaces the code of the request before (ADR 0112). "Cancel" forgets the key of this device for the server. It asks no question: the key belongs to no account yet. The server stays in the server book without an account.
- A link code in another form than ADR 0110 gives is not shown (ADR 0714). The view then says that a new code is needed.
- The application checks a waiting link every 5 seconds (ADR 0730). The device logs in without an action of the person, and the dialog closes by itself.

One server, the logged-in device:

- The devices page of the account layer takes the code and approves the key (ADR 0781).

All servers:

- The settings of the application have a section "Devices" with two buttons: "Link a new device to all my servers" and "Link this device to my servers". Each opens one layer in that role. The layer belongs to no server.
- The first device shows its offer text with "Copy", and a field for the request text of the new device. "Look up" runs the lookup of ADR 0716 and approves nothing. The layer then shows the fingerprint, the name that the new device gave, and one line per server with the account that the new device is added to and the progress: working, ready, approved, failed with the reason, unknown, or account changed with the button "Use this account". "Approve" approves every link that is ready.
- The account of a line is named by the name of the account of this device on that server, when the state of the server holds it, and by its identifier otherwise.
- The sentence of "account changed" is fixed text. The account that the device has now is named in a line of its own under the sentence, which is cut at the width of its place (ADR 0781).
- The new device has a field for the offer text and one for its device name. "Request links" runs the start of ADR 0716. The layer then shows the request text with "Copy", the fingerprint, and one line per server: working, waiting, linked, or failed with the reason; and "Check" and "Try again".
- A line changes when the result of its server arrives, while the action still runs for other servers.
- The line of a server on the new device also follows the state of that server: the application checks a waiting link by itself, so a server that is logged in is shown as linked before "Check" is pressed, and a link whose code expired is shown as failed.
- An action that ended with failed servers states the count: "1 of 3 servers did not succeed."
- "Start over" drops the flow of the layer and starts a new one.

The two texts:

- A text is given to the clipboard by the click on "Copy" and at no other moment.
- A field for a text takes at most 32 KiB (ADR 0716). A longer text is removed from the field, and the layer says so.
- The request text holds link codes. It is held by the flow and by the view of the layer, in a type whose formatted form shows nothing. It is not the accessibility label of its element. The field of the first device is emptied when the lookup starts, and every field of the layer when the layer closes. The copies that the drawing of a text makes are freed with the frame and are not erased, and a copy on the clipboard stays until the person replaces it (ADR 0761).
- The layer draws the first 480 characters of a text and states the length of every text, also of one that it draws whole. "Copy" takes the whole text.

## Consequences

- A person links a second installation without the command-line tool: with one code and one comparison for one server, or with two texts and one comparison for all servers.
- The comparison on one server has 43 characters in eleven groups. It is longer than a fingerprint and leaves no key that differs and looks the same.
- A layer that is closed while its action runs does not stop the action (ADR 0712). Each server then shows its own state: waiting for a link with its code, or logged in.
- A person who presses "Cancel" at the moment of the approval leaves a device on the account whose key no installation holds. The devices page lists it, and it can be removed there.
- The flow of the new device asks a server while the application may check the same server by itself. The flow waits for that check and then runs its own action, so "Check" and "Try again" are not refused as "busy" (ADR 0712).
- The offer shows which servers the person is on to whoever reads the text (ADR 0716).
