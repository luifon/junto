# 0782. Device linking in the window

## Status

Accepted

## Context

A second device gets access to an account through a link that a logged-in device approves (ADR 0104, ADR 0112). One flow links a new device on the servers that the person chooses, with one code and a relay on the first server (ADR 0716). The client core has the flows of both sides (ADR 0712), and the command-line tool runs them. The approval names a public key (ADR 0112), so both devices must show that key in one form that a person can compare.

## Decision

The key that a person compares:

- Both devices show the public key of the new device on the first server as the "Device code". Its text is the text that the command-line tool prints for a key, unpadded base64url with 43 characters, written in groups of four characters. One function of the text catalog makes it, and both sides use it.
- Reason: a device with the command-line tool and a device with the window can be compared, and the text is the whole key and not a shortened form of it.

One layer, two roles:

- The layer of a link belongs to no server. Its role "Link a new device" approves a new device; its role "Link this device" makes this device the new one. The section "Linked devices" of the settings opens either role.
- The first start offers two choices: "I am new to Junto", which asks for the name of the preset (ADR 0717), and "I use Junto on another computer", which opens "Link this device". A device that chose the second keeps an empty preset until the person sets it; the welcome dialog opens at a start only while the preset is empty and no server is saved.
- The third way of the dialog "Join a server", "I already have an account: link this device", opens "Link this device" in place of the dialog and starts it with the address of that server. The server is saved when the person chooses this way, as with the other two (ADR 0732).
- The button "Link a new device" on the devices page of the account layer opens "Link a new device" with only that server on; the person can turn the others on.

Link this device:

- A field for the address of one server of the account, and one for the device name, which starts with the device name of the application (ADR 0730). "Get a code" runs the start of ADR 0716.
- While the first server waits, the layer shows the name of the server, the link code in large fixed-width type, the device code, the time at which the code expires in UTC, and what the person does on the other device. A second sentence says that a person without a device of the account at hand gives this key to the server owner, who can recover the account onto this device (ADR 0761). "New code" asks the first server again, which replaces the code before.
- The main area of a server that waits for a link shows the same code with "New code" and "Cancel"; "Cancel" forgets the key of this device for the server and asks no question, because the key belongs to no account yet.
- A link code in another form than ADR 0110 gives is not shown (ADR 0714). The view then says that a new code is needed.
- The application checks a waiting link every 5 seconds (ADR 0730). When the first server logged this device in, the window runs `check` of the flow by itself: the layer tells that this device is linked on that server, reads the offer, and shows one line per other server: working, waiting, linked, or failed with the reason. The window also runs `check` when the code of a waiting server expired. A line follows the state of its server, so a server that logged in shows as linked before the flow is asked. "Try again" runs `check`; when every line is linked, "Done" closes the layer.

Link a new device:

- One line per server of this device with a switch, all on; a server that cannot be linked is off with its reason and cannot be turned on. A field for the code of the new device; "Find the device" looks the code up on the servers that are on and approves nothing. An empty code and a code of more than 16 characters are not sent.
- The layer then shows the device code, the name that the new device gave (on a line of its own, cut at the width of the page), and the servers; the first server is on and cannot be turned off, with "The link starts on this server." "Approve" approves the new device on the first server with the other servers that are on.
- The layer then shows one line per server with the account that the new device is added to, named by the name of the account of this device on that server when the state of the server holds it and by its identifier otherwise, and the progress: waiting for the new device, working, approved, already linked, or failed with the reason. The window runs `check` when the first server tells that the relay changed. "Try again" runs `check`; when no server waits, "Done" closes the layer.
- An action that ended with failed servers states the count: "1 of 3 servers did not succeed."

Common:

- "Start over" drops the flow of the layer and starts a new one in the same role. Closing the layer drops its flow; an action that runs goes to its end in the client core (ADR 0712), and each server shows its own state afterwards.
- The fields of the layer are emptied when the layer opens and closes. The code that the device that approves typed is held by its flow from the lookup on, and the field is emptied then.

## Consequences

- A person links a second installation without the command-line tool, with one code and one comparison.
- The comparison has 43 characters in eleven groups. It is longer than a fingerprint and leaves no key that differs and looks the same.
- A layer that is closed while the new device waits does not stop the link of the first server: that server logs this device in by itself. The other servers are then linked only with a new link.
- A person who presses "Cancel" at the moment of the approval leaves a device on the account whose key no installation holds. The devices page lists it, and it can be removed there.
- The flow of the new device asks a server while the application may check the same server by itself. The flow waits for that check and then runs its own action, so `check` is not refused as "busy" (ADR 0712).
