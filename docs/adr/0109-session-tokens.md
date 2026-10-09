# 0109. Session tokens

## Status

Accepted

## Context

After a login (ADR 0108) each request must show that it comes from the logged-in device without a new signature. ADR 0104 requires that removing a device ends the sessions of that device, and ADR 0106 requires that a ban ends the access of an account. A token that the server only verifies by signature cannot be ended before its expiry.

## Decision

- A session token is the text `jst_` followed by 32 random bytes in base64url without padding. The bytes come from the random source of the operating system.
- The token has no content that a client can read.
- A request carries the token in the header `Authorization: Bearer <token>`.
- The server stores the SHA-256 hash of the token and never the token. A lookup finds the row by the hash and then compares the stored hash and the computed hash in constant time.
- A session belongs to one device and lives 30 days from the login. It is not extended by use.
- A request is accepted only when the session exists, has not expired, and the account of its device is active. The server checks this inside the transaction in which the request then reads and writes (ADR 0009), so a ban that commits first ends the request.
- A logout deletes the session. Removing a device deletes the device and, with it, its sessions. A ban deletes every session of the account. A removal deletes the devices of the account. A recovery deletes every session of the account (ADR 0112).
- A login creates a session only after it has read the active state of the account in the same transaction.
- The maintenance task deletes expired sessions (ADR 0009).
- A token is not written to a log. The type that holds it prints `redacted` in debug output.

## Consequences

- A copy of the database does not contain a usable token.
- The end of a session takes effect at the next request, and for a request that is in progress, at its commit.
- Each authenticated request costs one database lookup.
- A client logs in again at least every 30 days. The login needs no action by the user, because the device signs the challenge.
- An access token for automation (ADR 0203) is a separate credential and is not defined by this record.
