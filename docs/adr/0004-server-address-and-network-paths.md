# 0004. Server address and network paths for calls

## Status

Accepted, pending step 0

## Context

A client needs an address that identifies a server and lets the client verify the server's identity. A TLS certificate needs a hostname, and an IP address changes when the server moves to another host. Call media is sent over UDP, which some networks block. Three services of one deployment need port 443 on the same public address.

## Decision

- A server address is a domain name with a valid TLS certificate.
- The client accepts a plain IP address only for local development.
- Call media uses UDP on one multiplexed port or on a port range.
- A TCP port is the fallback for media when UDP is blocked.
- Three services use port 443 on the public address: the server's HTTPS, LiveKit's signalling, and TURN over TLS.
- Each of the three services has its own hostname. A router in front of them selects the service by server name.

## Consequences

- An operator needs a domain name, three hostnames and a valid certificate for each.
- A server keeps its address when it moves to another host, because the address is a name.
- The public host must run a router that selects by server name on port 443.
- Step 0 tests that a call works when the network allows only TURN over TLS on port 443 (`docs/step-0/README.md`, test 5).
