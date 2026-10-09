# 0004. Server address and network paths for calls

## Status

Accepted, pending step 0

Open: a call on a network that allows only TLS on port 443 was not run. The entry on port 443 with three server names and TURN over TLS exists as a written configuration and was not applied (`docs/step-0/README.md`, test 5, first check). The UDP path and the TCP fallback are tested.

## Context

A client needs an address that identifies a server and lets the client verify the server's identity. A TLS certificate needs a hostname, and an IP address changes when the server moves to another host. Call media is sent over UDP, which some networks block. Three services of one deployment need port 443 on the same public address.

Step 0 measured the paths without TLS (`docs/step-0/README.md`, test 5): a call over one UDP port worked with no lost packet. With all outbound UDP blocked at the client, a client based on libwebrtc connected through the TCP port and exchanged audio and video with no lost packet in 30 seconds. A client based on the LiveKit Go SDK did not connect through the TCP port.

## Decision

- A server address is a domain name with a valid TLS certificate.
- The client accepts a plain IP address only for local development.
- Call media uses UDP on one multiplexed port or on a port range.
- A TCP port is the fallback for media when UDP is blocked. The fallback is established for the client of ADR 0701, whose call implementation is based on libwebrtc. Tests of the fallback use a participant built on the LiveKit Rust SDK, not a tool built on the Go SDK.
- The media ports are open for IPv4 and IPv6. Clients used IPv6 for media when they had it.
- Three services use port 443 on the public address: the server's HTTPS, LiveKit's signalling, and TURN over TLS.
- Each of the three services has its own hostname. A router in front of them selects the service by server name.

## Consequences

- An operator needs a domain name, three hostnames and a valid certificate for each.
- A server keeps its address when it moves to another host, because the address is a name.
- The public host must run a router that selects by server name on port 443.
- A network that blocks UDP and the TCP media port, and allows only port 443, depends on TURN over TLS. That path is not tested, so the first version cannot yet state that a call works on such a network.
- TURN relays media through the media host. The CPU cost of the relay was not measured (ADR 0003).
