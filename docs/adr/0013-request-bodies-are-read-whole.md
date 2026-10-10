# 0013. A request body is read whole before the answer, up to a bound

## Status

Accepted

## Context

A route answered a request whose body was larger than the largest accepted body as soon as it had read that many bytes. The server then closed the connection while the client still sent. The operating system resets a connection that is closed with unread data, and a reset can discard the answer before the client reads it. One test saw this once: a client that sent 3 MiB got a broken pipe and no error body. A run without the rule of this record gave 1 of 12 clients a connection reset for a body of 48 MiB. ADR 0205 states that every error has the error body.

Reading a body before the route runs means that the server holds bytes for a request that has passed no authentication and no limit of a route. Without a bound, 500 requests that each send nearly 2 MiB and withhold the end of the body hold about 1 GiB until their time limits.

## Decision

- The server reads the body of every request before the route runs.
- A body has at most 2 MiB (2 097 152 bytes).
- A larger body is read to its end and discarded. Then the request gets `invalid_request` with a message that states the limit. The route does not run.
- The server reads at most 16 MiB of one body. After that it answers and closes the connection.
- The time limit of a request (ADR 0008) covers the reading of its body.
- The server holds at most 64 MiB of request bodies at one time, and at most 4 MiB for one client address (the client address of ADR 0111: the forwarded address when the request comes from a trusted proxy, and the /64 of an IPv6 address). A body counts while its request is read and while its route runs.
- A request whose next bytes would pass the bound of its address gets `rate_limited` (429). One that would pass the bound of the server gets `busy` (503). Both have `Retry-After: 1` and are answered at once, while the body still arrives. The bytes of that request are given back.
- Only `POST`, `PUT` and `PATCH` have a body in this API. The body of a request with another method is read to its end in the same way, and none of its bytes are held.
- A body that passes 2 MiB is no longer held from that byte on: the server discards what it has and what follows.
- A route with a smaller limit of its own checks it on the bytes that it received. The avatar route does this (ADR 0119).

## Consequences

- A client that sends a body of up to 16 MiB receives the error body, not a broken connection.
- A client that sends more than 16 MiB can still see a reset. No client of the product sends such a body.
- The server holds at most 2 MiB of one request in memory while it reads it, and at most 64 MiB for all requests. The routes that read JSON held the same bytes of one request before.
- A request that is refused for a bound can lose its answer to a reset, because the server does not read the rest of its body. Reading it would cost the time that the bound saves. A client that respects the bounds does not get this answer.
- Thirty-two requests of 2 MiB that are read at the same moment fill the bound of the server, and further requests with a body get `busy` until one ends. The largest bodies of the product are avatars of at most 256 KiB (ADR 0119).
- Many clients behind one address that is not a trusted proxy share the 4 MiB of that address.
- The work of reading and discarding a body that is too large is bounded by 16 MiB and by the time limit of a request, not by a count of such requests.
- A request that sends its body slowly gets `timeout` after the time limit of a request.
