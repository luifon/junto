# 0003. Deployment with Docker Compose in two forms

## Status

Accepted, pending step 0

Open: the public entry on port 443 with three server names and TURN over TLS was not run (`docs/step-0/README.md`, test 5, first check), and the capacity of a media host that also carries that entry and TURN was not measured (test 6). The recorder in the two-host form is tested (test 3).

## Context

The components of ADR 0001 run on hardware that the operator controls. PostgreSQL and Redis must not be reachable from the public network. The media server needs a public address. Some operators have one host with a public address; other operators keep the data on a host that cannot receive inbound connections and add a second host that has a public address. The recorder needs several CPU cores while it records, which a small public host may not have.

Step 0 measured the following (`docs/step-0/README.md`, tests 3, 5 and 6):

- The LiveKit server needs Redis for running calls, with one node too. Calls stopped 6 seconds after Redis stopped, and new joins and the server API failed.
- A recorder on a host that cannot receive connections recorded calls of a LiveKit server on another host. It reached the LiveKit server on its public address and reached Redis through a private link.
- With the private link down for 30 seconds, calls and recordings continued and every recording file was complete.
- A port that Docker publishes was reachable before any rule for it existed in the host firewall.
- A plain restart of the recorder container ended the recorder after about one second and lost every recording in progress.

## Decision

Containers:

- The system is deployed with Docker Compose, with a separate container per component.
- PostgreSQL runs on an internal Docker network with no published port. Only the server container is on that network.

Redis:

- Redis is part of the call path. It runs on the host of the LiveKit server, with a restart policy and a health check.
- Redis requires a password. Its port is not published, or is published on the loopback address only. A rule of the host firewall does not protect a port that Docker publishes.
- In the two-host form Redis also accepts connections on the address of the private link, and on no other address.

Forms:

- Two forms are documented, each with its own Compose file:
  1. **One host with a public address.** This is the default form. All components run on that host.
  2. **Two hosts.** The data host runs the server, PostgreSQL, the file storage, the recorder and the transcription worker. The public host runs the LiveKit server, Redis and the public entry on port 443. A private network link joins the two hosts.
- In the two-host form, the public host accepts HTTPS for the server and forwards it over the private link. The server listens on a local port.
- In the two-host form, LiveKit is configured with its public address.

Private link:

- The data host opens the private link and keeps it alive, because only the public host can receive connections.
- The link is WireGuard in userspace, in one container on each host. It needs no kernel module. An operator can use the kernel implementation instead.
- The link container waits for the control socket of the userspace process before it calls the WireGuard configuration tool. A call before that makes the kernel load its own WireGuard module.
- Each side of the link allows one peer address. Neither container forwards IP packets.

Recorder:

- The recorder runs on the data host. It can therefore run on a host that cannot receive inbound connections.
- The recorder connects to the LiveKit server on its public address, for signalling and for media, as a client does. It uses the private link only for Redis.
- The recorder container is not in the network namespace of the link container, so a loss of the link does not interrupt its media path.
- The recorder is stopped only after the server has ended the recordings, and with a stop timeout that is longer than the time a recording needs to finish (ADR 0601).
- The log level of the recorder stays above debug, because its debug log contains the room token of the recorder.

Media host:

- The LiveKit server and Redis have a restart policy.
- The media host sets the kernel limits for UDP receive and send buffers to the 5,000,000 bytes that LiveKit asks for. With the default limits the host counted UDP send-buffer errors under load.
- The firewall opens the media ports for IPv4 and IPv6 and keeps the metrics port of LiveKit closed. With host networking that port listens on all addresses.

## Consequences

- In the two-host form the private link is required from the first version, because the recorder uses it for Redis.
- In the two-host form the server is unavailable while the network connection of the data host is down.
- A loss of the private link of 30 seconds interrupts neither calls nor recordings. During the loss a stop request to the recorder is not delivered, and a state change of the recorder reaches the LiveKit server after the link returns: 16 seconds after it in the measured case (ADR 0601). A loss longer than 30 seconds was not tested.
- A stop of Redis ends every call within seconds. After Redis returns, a client whose SDK has ended its reconnection attempts is no longer in the call, so the client joins again by itself.
- The host that runs the recorder needs CPU cores that no other component takes while a recording runs. The measured use for one recording is in ADR 0602. The transcription worker on the same host must not take those cores during a recording.
- The capacity of a media host depends on its hardware and is measured per deployment with test participants. The load points that step 0 measured are in `docs/step-0/README.md`, test 6. A recording did not raise the CPU use of the media server by a measurable amount. The CPU use follows the number of packets and of connections, so many participants who send audio at the same time cost more than a few video streams of the same total bitrate.
- Whether a call admission check runs while the private link is down depends on the path of the server's requests to the LiveKit API (ADR 0506). Requests to the LiveKit API and signalling through the link were not tested.
- Redis 7.2 is the last version line under the BSD licence. The Redis version, or a compatible server, is chosen with the licence check of ADR 0801.
