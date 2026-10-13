# 0101. Independent servers without federation

## Status

Accepted

## Context

Any person can run a Junto server, and one person can be a member of several servers. Two designs make this possible: servers that exchange data with each other (federation), or servers that are independent while the client connects to each of them. Federation requires a protocol between servers, shared identities and trust between operators.

## Decision

- Servers never communicate with each other.
- Each server is independent. Other people can run their own servers.
- One client connects to several servers.
- The client keeps a server book: the list of the addresses of the servers it has saved.
- The client uses each server's own API (ADR 0201).
- Each server exposes a public endpoint that returns its name, its logo and the API versions it supports.

Logo:

- Administrators with a session set the logo with `PUT /api/v1/server/logo` and remove it with `DELETE`. No rank comparison applies. Access tokens cannot change it.
- The body is an image under the same byte, dimension, format and animation checks as an avatar (ADR 0119). Changes share that account's profile-change limit. One bounded PostgreSQL row holds the bytes, hash and dimensions.
- `GET /api/v1/server/logo` is public and returns the image with its detected media type, SHA-256 ETag, `nosniff` and `public, no-cache`. An absent image is `not_found`. Removed and banned people may still read it, as anyone may before joining.
- The transaction updates `logo_path` in the public server information, records `server_logo_changed` for active accounts at both stream levels, and records the audit action of the same name for server audit readers. Each carries optional avatar metadata; neither contains image bytes. Removal uses null. An identical upload or an already absent removal records nothing. A reconnect can reload the public server information and logo.

## Consequences

- There is no protocol between servers and no trust relation between operators.
- An account, a direct message, a search and a call exist inside one server only. A person on two servers has two accounts, one per server (ADR 0103).
- The client holds state and a connection for each saved server (ADR 0107), and client state is built for several servers from the first build step (ADR 0703).
- The client can show the name and logo of a server before the person has an account on it.
- Servers can run different versions. The client handles the API version of each server separately (ADR 0201).

Public logo reads are limited to 120 requests per minute per client address, with the proxy and IPv6 rules of ADR 0111. A matching `If-None-Match` (including weak tags, lists and `*`) returns 304 with the ETag and cache policy without loading the image bytes. Cross-origin embedding stays permitted because the logo is public server identity available before joining; it does not reveal membership. A web page can consequently observe whether the public image exists at an address it can request.
