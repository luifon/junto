# 0716. Linking a new device on several servers with one code

## Status

Accepted

## Context

ADR 0104 states that one flow links the servers of the first device: the new device creates a device key per server, and the first device approves each one. ADR 0112 defines the link on one server, with a code that the new device gets and that the user types into a logged-in device. With several servers the new device must learn which servers to add, each server gives its own code, and one server can fail while the others succeed. Servers do not know each other (ADR 0101) and do not trust each other (ADR 0714). An earlier form of this flow had the user carry two long texts between the devices; the user now types one code, and the first server carries the rest between the two devices.

## Decision

The first server:

- The new device adds one server of the account by its address and asks it for a link code (ADR 0112). That server is the first server of the link.
- The device that approves (the old device) lists its servers with a switch each, all on by default; a server where it is not logged in is off and cannot be turned on, with the reason. The person types the code once. The old device looks the code up on every server that is on, at most four at a time; the first server in the order of the list that knows the code is the first server. It stays on and cannot be turned off, with the note that the link starts there. To link only other servers, the new device starts from one of them.
- The old device shows the device name and the key of the new device on the first server as the device code (ADR 0782). The person compares it once, for the first server only, and approves.
- The approval on the first server carries the offer: the identifier and the address of each other server that is on. The server keeps it in the relay of the link (below), created in the same transaction as the device. A server that does not state `capabilities.device_link_relay` gets no offer: only it is linked, and each other server of the list fails with "the first server cannot pass the other servers to the new device".

The relay:

- The relay is a record of its own on the first server, keyed by the new device. It holds the account, the old device, the new device, the offer, the latest answer of the new device and a count of answers. It survives the approval, which deletes the waiting link, and ends when either device ends it or 15 minutes after the approval, whichever is first; the maintenance task removes it then (ADR 0009). A removal of either device removes it.
- Only its two devices reach it, each with the session of its device; an access token does not. The new device reads the offer (`GET /api/v1/account/device-link-relay`) and writes its answer (`PUT /api/v1/account/device-link-relay/answer`); the old device reads the answer (`GET /api/v1/account/device-link-relays/{device_id}`); either ends it (`DELETE` on the same path). A message has at most 64 KiB, and the new device writes at most 256 answers.
- The creation and each answer record `device_link_relay_changed` with the new device for the account (event revision 23), so the old device reads the answer when it changes and polls nothing. The new device reads the offer when its first server logged it in.

Sealed messages:

- The first server must not be able to make the old device approve a key of its own choice on another server, and must not read the list of the other servers. Each message is signed by the sending device and encrypted to the receiving device, with the keys of both devices on the first server.
- The X25519 key of a device is derived from its Ed25519 device key (ADR 0711). A sealed message is a fresh X25519 exchange key followed by the ChaCha20-Poly1305 encryption, under a key from HKDF-SHA256 of the exchange (salt: the exchange key and the Ed25519 key of the receiver; information: the version and the kind of message), of the Ed25519 signature and the JSON content. Each key encrypts one message, so the nonce is zero. A shared secret of zero (a key of small order) is refused. The signature covers the version, the kind (offer or answer), the first server and the key of the new device there, so a message of one relay is not accepted as a message of another.
- The old device accepts an answer only with the signature of the key of the new device that the first server added, which is the key that the person compared. It also refuses an answer when the key that the server names for the new device differs from that key.
- The new device verifies the offer with the key of the old device that the first server names. The person compared no key of the old device, so the first server could replace the offer; such an offer can only make the new device add servers, and each added server must answer as the identifier of the offer (below), and nothing is approved without the old device.
- The two uses of one key, signature and exchange, are separated by the HKDF information and the signature context; the same construction is analysed in "On using the same key pair for Ed25519 and an X25519 based KEM" (eprint 2021/509).

The new device:

- The flow has the states `enter address`, `waiting` (the first server) and `linked` with one line per server of the offer: `working`, `waiting`, `linked`, or `failed` with the reason.
- `start` takes the address and a device name, adds the server when it is not saved, and asks it for a link code. A device that already has an account there is refused. `new code` asks the first server again.
- `check` asks the first server whether it linked this device. After that it reads the offer, adds each server of the offer that is not saved (the address must answer as the identifier of the offer, ADR 0714), asks it for a link code with a key created for it, and writes the answer: for each server its code and key, `linked` when this device has an account there already, or `failed` with the reason. Later checks ask every waiting server whether it linked this device; a server that failed and a server whose code expired are asked again, and the answer is written again when it changed.

The device that approves:

- The flow has the states `enter code` (the list of servers with their switches), `confirm` (the first server, the device name, the device code) and `linking` with one line per server: `waiting` for the new device, `working`, `approved`, `already linked`, or `failed` with the reason.
- The account of a line is the account of this device on that server when the person approves; every request of the line is sent for that account, and a change of the account fails the line (ADR 0712).
- `check` reads the answer and, for each server with a code that it has not tried, looks the code up on that server and approves only when the server holds the key of the answer behind the code. A line that failed is tried again when the new device answers with another code. When every line is approved or already linked, the old device ends the relay.

Results per server:

- The servers are worked on side by side, at most four at a time, in every step. No server waits for the end of another, and a server that does not answer holds up none.
- The line of a server changes when its result arrives, while the action still runs for other servers, and the interface is told of each change.
- An action over several servers ends with the error "incomplete" and the count of failed servers when at least one failed. The state still holds the result of every server.

## Consequences

- A person with several servers links a new device with one code and one comparison.
- A link code alone still gives no access (ADR 0110). The first server can neither read the list of the other servers nor have a key of its choice approved on them.
- The new device learns the addresses of the servers that the person turned on, and only these.
- A server that is unreachable during the flow fails alone; `check` on the new device asks it again while the relay lives. After the relay ended, the server is linked with the flow for one server.
- A relay that the old device does not end lives 15 minutes; a waiting link code lives 10 minutes (ADR 0112) and the new device asks for a new one when it expired.
- With more than four servers, a fifth starts when one of the first four has ended.
