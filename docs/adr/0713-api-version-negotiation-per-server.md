# 0713. API version negotiation per server

## Status

Accepted

## Context

ADR 0201 states that each server reports the API versions it supports and that the client supports the current version and the previous one. Servers are upgraded at different times (ADR 0101), so one client holds servers of different versions at the same moment. The records do not state which version the client picks, when it picks, or what a server without a common version looks like in the client. Only version 1 exists today.

## Decision

- A build of the client names two versions: its current version and, when one exists, the version before it. The current version of this build is the version of the protocol crate, and version 1 has no version before it.
- The client reads the versions of a server from the route `GET /api/server`, which has no version in its path (ADR 0205).
- The client picks the current version when the server reports it, and the previous version otherwise. It picks no other version.
- The pick is made per server, at the first contact after a start and again at each `check` of the access flow (ADR 0712). A server that was upgraded or downgraded is used with the new pick from then on.
- The picked version decides the route prefix `/api/v<version>` of every later request to that server. No request of one server uses the version of another.
- When there is no version in common, the client states why:
  - the oldest version of the server is newer than the current version of the client: the server needs a newer client;
  - the newest version of the server is older than the oldest version of the client: the server is too old for this client;
  - otherwise: no version in common;
  - the server reports no version.
- Such a server stays in the server book and is shown as incompatible with that reason. The client sends it no request except the server information, and it holds no session for it. Every other server is unaffected.
- A server with no common version can still be saved (ADR 0712), so that it works after an update of the client or of the server.
- The versions that a build supports are a value that is given to the client when it is opened. The tests open a client that names version 2 as current and version 1 as previous, and run it against a server that reports both, against one that reports only version 1, and against one that reports only later versions.
- A second API version adds, next to the route prefix, a module that translates between the wire types of the older version and the types that the flows use. This record does not define that module, because no second version exists.

## Consequences

- The client and a server always use the newest version that both speak, with a distance of at most one version.
- A server that is two or more versions away from the client cannot be used, and only that server (ADR 0201, ADR 0704).
- The reason "the server needs a newer client" is what the interface shows next to the update notice (ADR 0704), and "the server is too old" is the warning for an administrator.
- A downgrade of a server between two contacts is noticed at the next `check`, not at the next request. A request to a version that the server no longer serves gets `not_found` until then.
- The mechanism is tested with a pretended second version. The translation of types is not tested, because it does not exist yet.
