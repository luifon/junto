# 0716. Linking a new device on all servers in one flow

## Status

Accepted

## Context

ADR 0104 states that one flow links all the servers of the first device: the new device creates a device key per server, and the first device approves each one. ADR 0112 defines the link on one server, with a code that the new device gets and that the user types into a logged-in device. With several servers the new device must first learn which servers exist, each server gives its own code, and one server can fail while the others succeed. Servers do not know each other (ADR 0101), so the two devices exchange everything themselves, and the user carries it.

## Decision

Two texts:

- The first device gives a link offer: the identifier and the address of every server on which it has an account. The offer holds no secret.
- The new device gives a link request: its device name, and for every server one link code and the public key that it created for that server.
- Each is one line of text with a fixed prefix that states its kind and version, followed by JSON in base64url. A text has at most 32 KiB and names at most 64 servers, each once.
- A link code in a request has the form of ADR 0110 (ADR 0714). The limit of 32 KiB is checked when the request is built: a server whose link would break a rule fails alone and is left out of the request, and the other servers stay in it.
- The user carries the two texts between the devices. How they travel (typing, a file, a picture) is not part of this record.

The new device:

- The flow has the states `enter offer` and `requested`. `requested` holds one line per server with its progress: `working`, `waiting`, `linked`, or `failed` with the reason.
- `start` takes an offer and a device name. For every server of the offer it adds the server when it is not saved, asks its address for the server information, and refuses the server when the reported identifier is not the one of the offer. Then it asks the server for a link code (ADR 0112), with a key that is created for that server and that address (ADR 0711).
- `requested` shows the link request with every link that waits, and a fingerprint.
- `check` asks every server that waits whether the device is linked. `retry` asks again on every server that is not linked: a failed server gets another try, and a waiting server gets a new code. The request then holds the links that wait now.

The first device:

- The flow has the states `enter request` and `confirm`. `confirm` holds the fingerprint, the device name, and one line per server with the account of this device there, the public key and the progress: `working`, `ready`, `approved`, `failed` with the reason, `unknown`, or `account changed` with the account that the device has now.
- `look up` takes a link request. For every link it finds the server in its own server book by the identifier. The address comes from the book, never from the request. A server that is not saved fails with "unknown server".
- For every link it reads the devices of its account and the public key behind the code on that server. A device with the public key of the request means `approved`. A public key behind the code that differs from the one in the request fails with `public_key_mismatch`. Otherwise the link is `ready`.
- The account of a server line is fixed at its first lookup and is what the user sees and confirms. It stays the same through every later step of the flow.
- Nothing is approved before the user acts. `approve` approves every link that is ready, each for the account of its line (ADR 0712). A link that failed after its account was shown is looked up again, for that same account, and approved in the same step, so `approve` is also how the failed servers are resumed. A link whose account was not shown yet, because its first lookup failed, is only looked up; the user sees its account and approves it with the next `approve`.
- When this device has another account on a server than the one of the line, nothing is sent for that server and its line becomes `account changed` with the new account. `approve` leaves such a line alone, however often it runs. The action `confirm account` for that server accepts the new account: the line gets it, the link is looked up again and becomes `ready`, and nothing is approved until the next `approve`.
- An approval whose answer was lost is resolved through the list of devices, as in the flow for one server (ADR 0712). When the list cannot be read, the link is `unknown`, and the next `approve` resolves it.

Results per server:

- The servers are worked on side by side, at most four at a time, in every step: asking for codes, checking, looking up and approving. No server waits for the end of another, and a server that does not answer holds up none.
- The line of a server changes when its result arrives, while the action still runs for other servers, and the interface is told of each change.
- A failure of one server changes the line of that server only.
- An action over several servers ends with the error "incomplete" and the count of failed servers when at least one failed. The state still holds the result of every server.

Fingerprint:

- Both devices show a fingerprint of the request: the first 8 bytes of the SHA-256 hash over a fixed first line and one line per server with the server identifier and the public key, sorted by server identifier, written as four groups of four hexadecimal digits.
- The user compares the fingerprint on the two devices before approving. The codes and the device name are not part of it.

## Consequences

- A person with several servers links a new device with two texts and one comparison, not with one code per server.
- A link code alone still gives no access (ADR 0110). A request that was replaced on its way shows another fingerprint, and a request with a key that differs from the key behind a code is refused by the client and by the server.
- An address in an offer that answers as another server is not added, and no key is created for it.
- The first device never contacts an address that a request names. A request cannot make it sign or send a token to a new host.
- The offer tells the new device the addresses of the servers of the first device. A person who reads the offer learns which servers the user is on.
- A server that is unreachable during the flow is linked later with `retry` and a second request, or with the flow for one server.
- A device is never added to an account that the user did not see next to its server. A device whose account changed during the flow needs one more confirmation for that server.
- With more than four servers, a fifth starts when one of the first four has ended. Four servers that do not answer delay the others by one request time limit.
- A request text holds link codes that live 10 minutes (ADR 0112). A text that is carried too late fails with "link code invalid", and `retry` gives a new one.
