# 0004. Server address and network paths for calls

## Status

Accepted, pending step 0

On 2026-10-16 the entry ran on a public address in the two-host form with ACME certificates for all three names. Certificate chain and hostname verification passed, including TURN with TLS 1.2 and TLS 1.3. A Linux participant restricted to TCP 443 and DNS failed to join: WebRTC rejected the valid TURN certificate with an unknown-CA error, while the system TLS verifier in the same Linux image accepted it. The pinned WebRTC build lacks an ISRG root in its compiled set and has no Linux system-trust fallback. Open: correct that trust path and pass a TCP-443-only call. Direct media worked; the UDP path and the TCP media-port fallback remain tested separately.

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
- The router obtains the certificates of the three names by ACME. For the requests of the certificate authority the public address also accepts TCP port 80, which otherwise answers with a redirect to HTTPS.

## Consequences

- An operator needs a domain name, three hostnames and a valid certificate for each.
- A server keeps its address when it moves to another host, because the address is a name.
- The public host must run a router that selects by server name on port 443. In the one-host form that host is the only host.
- The firewall of the public address opens port 80 in addition to port 443 and the media ports.
- A network that blocks UDP and the TCP media port, and allows only port 443, depends on TURN over TLS. Linux calls on that path remain unsupported after the certificate-trust failure observed on 2026-10-16; a trust correction needs a new test on that restricted network.
- TURN relays media through the media host. The CPU cost of the relay was not measured (ADR 0003).
