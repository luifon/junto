# 0108. Wire format of the login challenge and the limit on failed attempts

## Status

Accepted

## Context

ADR 0103 states that a device signs a challenge that is used once, expires quickly and names the server, the account, the device and the operation, and that failed attempts are limited. It does not fix the bytes that are signed, the lifetime or the form of the limit. Some operations happen before the device has an account: claiming the server (ADR 0201), joining (ADR 0105) and asking for a device link (ADR 0104). These also need proof that the caller holds the private key of the public key it presents. A limit that counts failures per device key lets any party that knows a public key lock that key out by sending wrong signatures.

## Decision

Keys and encodings:

- A device key is an Ed25519 key. A public key (32 bytes), a signature (64 bytes) and a nonce (32 bytes) are written in base64url without padding.
- A signature is checked with the strict Ed25519 verification, which refuses weak keys and non-canonical signatures.

Server identifier:

- The first start generates a random server identifier. The public server-information route returns it.
- A client stores the identifier with the server address and compares it with each challenge.

Challenge:

- A device asks for a challenge with its public key and an operation. The operations are `login`, `claim_owner`, `join`, `request_to_join`, `join_request_status` and `link_device`.
- The response carries the challenge identifier, the server identifier, the operation, the account identifier (absent when the key belongs to no account), the public key, the nonce and the expiry time in whole seconds.
- The device signs these bytes, built by the client from the fields:

  ```
  junto-challenge-v2
  server:<server identifier>
  origin:<origin that the device connected to>
  operation:<operation>
  account:<account identifier or "none">
  device:<public key>
  challenge:<challenge identifier>
  nonce:<nonce>
  expires:<expiry as seconds since 1970-01-01T00:00:00Z>
  ```

  The lines are joined with a line feed and there is no line feed at the end.
- The origin is not a field of the challenge. The device takes it from the address that it connected to, and the server accepts only one of its own public origins (ADR 0117).
- A request that needs proof carries the challenge identifier, the origin that the device signed, and the signature. The server reads the public key from the stored challenge.
- A challenge lives 60 seconds.
- The server stores one challenge per public key and operation, so at most 6 per key.
- A request for a key and an operation whose stored challenge is unused and has at least half of its lifetime left gets that same challenge again. Otherwise a new challenge replaces the stored one.
- Reason: a public key is not a secret. With a replacement at every request, a party that knows the key of a device ends each challenge of that key before the device answers it, and the logins of the device fail. A challenge is not a secret either: only the holder of the private key can answer it. A device answers within seconds, so a challenge is never replaced while a device still answers it in the first half of its lifetime, and no device gets a challenge that is about to expire.
- The server marks a challenge as used in the transaction of the operation that the challenge authorizes (ADR 0009). When that transaction commits, the challenge is used, and a second request with it gets `challenge_invalid`.
- When that transaction does not commit, the challenge is unused. This is the case after a wrong signature, after a refused operation, and after the errors `busy` and `timeout`. The client can send the same signed request again until the challenge expires.
- A route accepts only a challenge of its own operation.
- A `login` challenge is given only for a key of an active account. Another key gets the error `unknown_device`, and a key of a banned account gets `account_banned`.
- Expired challenges are deleted by the maintenance task (ADR 0009).
- The server counts the challenges that it issued and that have not expired. At 10 000 it refuses a further one with the error `challenges_full` and a `Retry-After` header that states when the oldest one expires. The count is held in memory, and a challenge that is replaced or used keeps its place until its expiry time. A request that gets the stored challenge again takes no place.

Failed attempts:

- A valid signature for an unexpired, unused challenge of the right operation always succeeds. No count of failures stops it.
- No count is kept per device key. An Ed25519 signature cannot be guessed, so a lock on a key protects nothing, and it would let any party that knows a public key deny its owner the login.
- A wrong signature counts against the client address (ADR 0111). After 10 wrong signatures in 10 minutes from one address, each further wrong signature from that address gets the error `too_many_attempts` with a `Retry-After` header until the window ends.
- An expired or unknown challenge does not count.
- Each attempt needs a new challenge, and requests for a challenge are limited to 120 per minute per client address (ADR 0111). This limit bounds the work that one address can cause.

## Consequences

- A signature is valid for one server, one origin, one account, one device key, one operation and one challenge. A client that builds the bytes itself cannot be made to sign text that a server chose freely.
- A signature that a device made for the address of one server is of no use at another server, also when the first server announces the identifier of the second (ADR 0117).
- A change of the signed bytes needs a new first line and a new API version. The origin line was added, with the first line `junto-challenge-v2`, before the first release of API version 1.
- A party that sends wrong signatures for the key of another person does not affect that person.
- The count of wrong signatures is held in the memory of the server process and is cleared by a restart.
- The response to a `login` challenge request shows whether a public key is registered and whether its account is banned.
- An address can hold at most 6 stored challenges per key that it names, each for 60 seconds, and can name new keys only at the rate of the challenge limit.
- The table of challenges holds at most about 10 000 unexpired rows, whatever the number of addresses and keys. A party with enough addresses can fill that number; then no device gets a challenge until places expire, which takes at most 60 seconds per place.
- A signed request that failed without a commit can be repeated unchanged. A request with a wrong signature can also be repeated with another signature for the same challenge; this gives no advantage, because a signature cannot be guessed.
- An account-less operation costs two requests: one for the challenge and one for the operation.
