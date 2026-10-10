# 0508. Media-server client, join tokens and room names

## Status

Accepted

## Context

The media server is LiveKit (ADR 0001). The server controls it and never carries media: it hands out join tokens, lists rooms and participants, removes a participant and changes what a participant may publish (ADR 0506). The tests of the server need a media server whose behaviour they control. A room needs a name, and a participant needs an identity that the clients in the room can map to an account. Step 0 found that a removal ends no token, and that the media server gives a connected client a token of 600 seconds (`docs/step-0/README.md`, test 5).

## Decision

Client module:

- The server reaches the media server through one trait with these functions: sign a join token, list the rooms, list the participants of a room, remove a participant, replace what a participant may publish, and verify the signature of a notification.
- The product uses the LiveKit implementation. The tests use a media server in memory that behaves as step 0 measured: it accepts a token for 60 seconds after its stated end, and a removal ends a connection and no token.
- The LiveKit implementation is the project's own code: four calls of the LiveKit room service over Twirp with JSON bodies (`ListRooms`, `ListParticipants`, `RemoveParticipant`, `UpdateParticipant`), and tokens signed with HMAC-SHA256.
- The official crate `livekit-api` 0.8.2 (Apache-2.0) was examined and is not used. Its service client adds a second version of the HTTP client library next to the one of the workspace, the generated types of every LiveKit service, and a token library, for four calls and one token format. The server needs its own trait for the tests, its own time limit per call and its own distinction between "unreachable" and "refused" in either case.
- Every request to the media server has a time limit of 2 seconds.
- A request is authorized with a token that the server signs for it: `roomList` for the list of rooms, `roomAdmin` for one named room.

Settings:

- `JUNTO_LIVEKIT_API_URL` is the address of the API of the media server. It starts with `http://`.
- In the first version the server reaches the API of the media server over a private network without TLS: the loopback interface or a container network in the one-host form, the private link in the two-host form (ADR 0003). The requests carry tokens signed with the API secret, never the secret itself.
- The server has no TLS client. An address with `https://` is refused at the start with a message that states this. The server does not start with a setting that it would silently not honour.
- `JUNTO_LIVEKIT_URL` is the address that clients connect to, with `wss://` or `ws://`.
- `JUNTO_LIVEKIT_API_KEY` and `JUNTO_LIVEKIT_API_SECRET` are the key of the media server. The secret is not printed and not logged.
- The four settings are given together. Without them the server has no media server, and no call can be joined.

Join token:

- A join token names one room and one identity, lets the participant subscribe, and lists the sources that the participant may publish: microphone, camera, screen, and the sound of a screen share.
- The list of sources is always explicit, so a track of an unknown source is refused.
- A join token does not let the participant send data messages, change its own metadata, create or administer a room.
- A join token has a stated lifetime of 60 seconds. The server treats it as usable for 120 seconds (ADR 0506).

Room name:

- A room belongs to one call. Its name is `jr-<namespace>-<32 random hexadecimal digits>`, with 128 random bits from the operating system.
- The name is made when the call begins and is not used again. The next call in the same voice channel has another name.
- The namespace is 12 hexadecimal digits of a hash of the server identifier. It tells the rooms of this server from other rooms on the same media server.
- A room name contains no identifier and no name of a channel, a workspace or an account.

Identity:

- The identity of a participant is `<account identifier>:<32 random hexadecimal digits>`. The second part is the seat.
- A client reads the account from the identity. The seat changes with every new place of the account in a call (ADR 0510).
- An identity contains no device identifier.

Notifications:

- The media server sends its notifications to `POST /internal/media/notifications`. The route is not part of the public API (ADR 0201) and is not in the OpenAPI document.
- The server accepts a notification only when its token is signed with the secret of the API key, has not ended, and carries the SHA-256 hash of exactly the received body. Another request gets `unauthenticated`.
- A notification changes nothing by itself. One that names a room of this server makes the admission check of that room run at once. One that names no room makes the next part of the full check run at once (ADR 0509).
- A notification is processed once. The server remembers each processed notification, by a hash of its body and of the end of its signature, until its signature is no longer accepted, and answers a repeated one with 204 without a check. It remembers at most 8192 notifications; a further one gets `rate_limited`.
- At most 300 notifications in 10 seconds start a check. A further one gets `rate_limited` (429) and is not remembered, so the media server can send it again.

## Consequences

- A token of an earlier call names a room that no later call uses. A person who was removed can return only for as long as the same call lasts.
- A token of an ended call can still create its old room on the media server. The admission check removes every participant of a room of this server that belongs to no call (ADR 0509).
- A change of the LiveKit API is found by the tests against a real LiveKit server (ADR 0902), not by a compiler. Those tests ran against LiveKit 1.13.9.
- In the two-host form (ADR 0003) the requests to the LiveKit API pass through the private link. No check runs while the link is down, which is the limit that ADR 0506 states.
- A deployment whose only path to the LiveKit API needs TLS cannot be built until the server has a TLS client. That needs a choice of a TLS library that passes the licence check (ADR 0801).
- An operator who puts the API of the media server on a network that others can read exposes the signed tokens of the requests, each valid for a short time, and the names of rooms and identities. The deployment documents state that the address must be private.
- The entry host does not need to forward `/internal/`. A caller from outside without the secret cannot make a check run.
- A caller who holds one valid notification, without the secret, can make one check of one room run with it, and no more.
- A notification that is refused for the limit is lost unless the media server sends it again. The full check finds the change at the turn of the room.
- Two accounts in one room see each other's account identifiers in the identities. They see each other in the call in any case.
