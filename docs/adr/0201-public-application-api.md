# 0201. One public application API, versioning and administration in the client

## Status

Accepted

## Context

The desktop client, the setup tool and scripts written by administrators all need to read and change the state of a server. A client that uses private endpoints leaves other programs with a smaller interface than the client has, and the private endpoints are not documented or tested as an interface. Servers are independent and are upgraded at different times (ADR 0101), so one client meets servers of different versions. Administration needs an interface, and a separate administration interface on the server would be a second surface to secure.

## Decision

One API:

- The server has one public application API: REST plus a WebSocket event stream.
- The client uses only this API. There are no private application endpoints.
- Call signalling and media go to LiveKit over LiveKit's own connection.
- LiveKit's callbacks to the server are internal and are not part of the public API.

Versioning:

- The API has a version number.
- Each server reports the versions it supports (ADR 0101).
- The client supports the current version and the previous version.
- Tests run the client against both versions (ADR 0902).

Administration:

- Administration happens in the client, through the public API.
- The server has a command line for two cases only. The first start prints a one-time code that makes the first client the server owner. A recovery of the server assigns a new server owner or issues a new one-time code for the owner (ADR 0114).

Not in the first version:

- Bot accounts, incoming webhooks and slash commands (`docs/scope.md`).

## Consequences

- Every function of the client is available to another program through the same API, under the same permission checks (ADR 0301) and with an access token (ADR 0203).
- The API documentation is generated and checked in CI (ADR 0202).
- A server that is two or more API versions away from a client cannot be used by that client. When a server requires a newer client, only that server is unavailable until the client is updated (ADR 0704).
- Access to the host of the server is needed only for the first start, for assigning a new server owner, and for backups and upgrades (ADR 0005).
- A person with access to the server command line can become the server owner.
