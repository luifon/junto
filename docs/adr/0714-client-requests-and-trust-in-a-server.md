# 0714. Client requests: addresses, proofs, repetition and what a server may cause

## Status

Accepted

## Context

The client talks to servers that other people operate (ADR 0101). A server can be wrong or hostile: it can answer with any text, any size and any status. The device key of one server must not become useful to another server, and nothing that a server sends may reach a file name or another host. ADR 0009, ADR 0108 and ADR 0205 state that a request can be sent again after `busy` and `timeout`, a signed request included, and leave the numbers and the case of a lost answer to the client. The HTTP library must work inside the future desktop client, whose own HTTP client runs reqwest 0.12 with rustls on a tokio runtime (`spikes/gpui-client/RESULT.md`).

## Decision

Server address:

- An address is an origin: scheme, host and port. A text without a scheme gets `https://`. A user name, a path, a query or a fragment is refused.
- The scheme is `https`. The scheme `http` and an IP address as host are accepted only for the loopback interface and for `localhost`, for local development (ADR 0004).
- Every request of a server goes to the origin of its entry. The client builds each path from fixed text and typed identifiers.

Identity of a server:

- The server identifier that the address reports when the server is added is stored with the address (ADR 0108, ADR 0710).
- At the first contact after a start and at each `check`, the client compares the identifier that the address reports with the stored one. When they differ, the client marks the server as "another server", drops its session and sends that address nothing but the request for the server information, until the stored identifier is reported again.
- A server identifier is saved once (ADR 0710). An address that reports the identifier of a saved server is not added.
- A key is stored with the address that it was created for and is given out for that address only (ADR 0711). An address that reports the identifier of a server whose key this device still holds, with or without a book entry, gets no signature of that key.

Proof with the device key:

- The client asks for a challenge with its public key and an operation (ADR 0108). It signs only after it has compared the challenge with its request: the server identifier is the stored one, the operation is the one asked for, the public key is the key of this device for this server, and the account is absent for every operation before a login.
- The client builds the signed bytes itself from the fields of the challenge. A challenge that fails a comparison is not signed, and the operation fails with the reason.
- The signed bytes name the origin of the address in the server book, and the proof carries the same origin (ADR 0117). The request for the challenge and the request with the proof go to that address. The client takes the origin from no response.
- A server address has the one spelling of an origin of the protocol crate. An address that the protocol type refuses, or writes in another way, is refused when it is typed and when it is read from a file.
- The client does not compare the expiry time with its own clock. The signature is valid for one server and one challenge, and the server enforces the lifetime; a wrong clock on the device must not prevent a login.
- A login challenge names the account that the server gives the key (ADR 0712). The session must be for that same account.
- One function signs. It takes a compared challenge and a key and refuses a key other than the one the challenge names.

Repetition of requests, in one place:

- After `busy` and `timeout` the client waits for the time of the `Retry-After` header, 1 second without the header and at most 5 seconds, and sends the same request again. A request is sent at most 5 times. Then the error is given to the caller.
- A signed request is sent again unchanged: the same bytes, so the same challenge and the same signature. The client asks for no new challenge between two attempts.
- A signed request whose answer was lost after the request may have arrived is sent again unchanged, at most 2 more times with 0.5 seconds between them. The challenge is used once at most, so the repetition cannot run the operation twice.
- When such a repetition gets `challenge_invalid`, or no answer arrives at all, the result is "outcome unknown". The flow then finds the truth with a login (ADR 0712).
- An answer to a signed request whose status says success and whose body cannot be read is also "outcome unknown": the server did the work and the client does not know its result.
- A request without a signature is never repeated after a lost answer.
- No other error is repeated by the client itself. A limit on attempts is given to the caller with its `Retry-After` time.
- An error body is read with the type of the protocol crate. A code that this client does not know is read as `unknown` (ADR 0209), and the text of the code is not kept. It is grouped by the HTTP status of its response: 401 and 403 as "not allowed", 404 as "not found", 409 and 410 as "conflict", 429 as "limited", 500 and above as "server fault", and every other status as "client fault". It is never repeated and never ends a session, also with the status 401.

What the client accepts from a server:

- The client follows no redirect. A redirect is an invalid response.
- A response body has at most 4 MiB. A request ends after 40 seconds, and a connection attempt after 10 seconds.
- The bound of one answer bounds no list that many answers fill. ADR 0747 bounds one message and what each list of messages holds.
- An error response is read as the error body of ADR 0205. Its message is cut to 200 characters without control characters and kept apart from the code. It is part of an error text of the client only for a code that the client does not know: that text names the status and quotes the message (ADR 0209).
- The name of a server and a device name that a server sends are cut to 64 characters without control characters before they are stored or shown by a flow. A logo path is used only when it resolves to the origin of the server.
- Other text that the API returns (names of accounts, roles and channels) is given to the caller as the server sent it. The interface treats it as text of another party.
- A session token is sent only in the `Authorization` header of a request to the origin of its server.
- The body of a request is a JSON document or the image of an avatar. The client sends an avatar only when it has at most 2 MiB and is a PNG, JPEG or WebP image by its first bytes; it is the cut of ADR 0119.
- An avatar that the client reads has the same two bounds. The client reads the format from the bytes and not from the `Content-Type` of the response, and computes the SHA-256 hash of the bytes itself. Another answer is an invalid response. The interface treats the image as a file of an unknown sender.

Secrets in memory:

- A session token is held in a type of this crate whose buffer is overwritten when the last copy is dropped. Copies of a token share one buffer. The token is read from the response into that buffer directly, without the session type of the protocol crate. That type erases its text as well (ADR 0209), but it accepts a token in any form and one whose JSON text has an escape, so the client keeps its own type for the token and for the response that carries it.
- A token is 1 to 512 letters, digits, `_` and `-` (ADR 0109). A token in another form is refused, and so is one whose JSON text has an escape, because the JSON reader would decode it in a buffer that this crate cannot overwrite.
- The body of a request and the body of a response are held in buffers that are overwritten when they are dropped. A buffer that must grow is copied to a new one and overwritten at once. A request that is sent again shares its body, and the HTTP library is given that same buffer and no copy of it.
- A link code that a server returns is accepted only in the form of ADR 0110: at most 32 digits, upper-case letters and hyphens. Another form is an invalid response, and the flow does not wait for a link.
- A code that the user typed is copied into the request body and held by the secret type of the protocol crate, which erases its text when it is dropped (ADR 0209).
- The HTTP library and the TLS library keep their own copies of headers, of response bodies and of what they send. This crate cannot overwrite them.

HTTP library:

- The transport is reqwest 0.12 with rustls, the `ring` provider and the certificates that the operating system trusts. This is the same library line and trust store as the HTTP client of the desktop framework.
- reqwest 0.13 with rustls depends on a certificate crate with the licence CDLA-Permissive-2.0 for a target that the client does not build. The licence check covers every target and the allowed list does not contain that licence (ADR 0801), so the client stays on 0.12 until the check or the list changes.
- The transport is behind an interface with one operation: send a request, get a response. The tests replace it with a server in memory.

## Consequences

- A hostile server gets no signature for another server, operation or key, and no session token of another server.
- A signature that this device made at one address is refused at every server that does not list that address as its own (ADR 0117). A server whose operator did not list the address that people use refuses every proof, and the client reports that cause.
- An error of a later server is shown by its status and by the message of the server as a quotation. The client does not show the text of a code that it does not know.
- A server cannot make the client write to a path of its choice, read a large body, or follow it to another host.
- A person who types an address without TLS gets an error, except for a server on the same machine.
- A server that moves to another host keeps its name and its identifier (ADR 0004). A server whose database was replaced reports another identifier and is treated as another server.
- A join, a claim or a request whose answer was lost does not create a second account and does not leave the client without its account.
- A request waits up to 20 seconds in total between its attempts when the server stays busy and names the longest wait each time.
- The client can be made to wait by `Retry-After` only up to 5 seconds per attempt.
- The workspace holds two reqwest lines: 0.13 for the tests of the server and 0.12 for the client.
- After a session ends, the memory of the client holds no copy of its token in a buffer that this crate owns. Copies inside the HTTP library can remain until their memory is used again.
- TLS verification depends on the trust store of the operating system. A server with a certificate from a private authority works when the user's system trusts that authority.
