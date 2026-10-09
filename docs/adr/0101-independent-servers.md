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

## Consequences

- There is no protocol between servers and no trust relation between operators.
- An account, a direct message, a search and a call exist inside one server only. A person on two servers has two accounts, one per server (ADR 0103).
- The client holds state and a connection for each saved server (ADR 0107), and client state is built for several servers from the first build step (ADR 0703).
- The client can show the name and logo of a server before the person has an account on it.
- Servers can run different versions. The client handles the API version of each server separately (ADR 0201).
