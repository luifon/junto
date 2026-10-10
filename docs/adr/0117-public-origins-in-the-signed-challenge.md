# 0117. The signed challenge names the public origin of the server

## Status

Accepted

## Context

A device signs a challenge that names the server by its identifier (ADR 0108). The identifier is public: the server-information route returns it. A hostile server can announce the identifier of another server. A client that keeps one device key per server identifier then uses the key of that other server at the hostile address. The hostile server asks the other server for a challenge for that key, passes it to the device, and sends the signature of the device to the other server. The other server accepts it, and the hostile server holds a session of the person there.

The device knows one thing that the hostile server cannot change: the address that the device connected to.

## Decision

Signed bytes:

- The bytes that a device signs contain the origin that the device connected to, as the line `origin:<origin>` after the line of the server identifier.
- The first line of the bytes is `junto-challenge-v2`. A signature over the bytes of ADR 0108 as first written is not valid.
- An origin is the scheme, the host and the port of the server address, as `https://chat.example.test` or `http://localhost:8080`. It has one spelling: lower case, no path, no trailing slash, and no port when the port is the default of the scheme. An IPv6 host is written in brackets in its shortest form.
- The client takes the origin from the address in its server book. It never takes it from a response of the server. The function of `junto-protocol` that builds the bytes takes the origin as an argument for this reason.

Proof:

- A request with a proof carries the origin that the device signed, next to the challenge identifier and the signature.
- The server refuses a proof whose origin is not one of its own public origins with the error `origin_rejected` (403). It does this before it reads the challenge, so the challenge stays unused.
- The server verifies the signature over the bytes with the origin of the proof.

Public origins of a server:

- The setting `JUNTO_PUBLIC_ORIGINS` lists the origins, separated by commas.
- A listed origin has the scheme `https`. An origin with the scheme `http` is accepted only for a loopback host: `localhost`, an address in 127.0.0.0/8, or `[::1]`.
- When the setting is given, only the listed origins are accepted. Loopback origins are not added.
- Without the setting, a server that listens on a loopback address accepts the loopback origins of its own port: `http://localhost:<port>`, `http://127.0.0.1:<port>` and `http://[::1]:<port>`. It logs a warning at its start.
- Without the setting, a server that listens on another address does not start.

## Consequences

- In the relay case the device signs the origin of the hostile server. The other server refuses that origin. When the hostile server replaces the origin in the proof, the signature does not match. A test runs both forms.
- A server behind an entry host needs its public origins in its settings. A wrong setting makes every login fail with `origin_rejected`, whose message names the setting.
- Local development needs no setting while the server listens on a loopback address.
- A server that is reachable from other machines never accepts a loopback origin, unless its operator lists one.
- The change of the signed bytes was made before the first release of API version 1, so the API version did not change. ADR 0108 requires a new API version for a later change.
- The rule covers every operation with a proof: login, owner claim, join, join request, its status, and device link.
