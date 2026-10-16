# 0003. Deployment with Docker Compose in two forms

## Status

Accepted, pending step 0

On 2026-10-16 the two-host form ran on two Linux hosts with a public entry, ACME certificates for all three names, a call, recording, CPU transcription, backup and isolated restore. A 30-second link loss preserved the call but left the recording `partly_failed`. The Linux TCP-443-only TURN call failed because WebRTC rejected the valid certificate (ADR 0004). Open: that TURN path, a public one-host installation, and the capacity of a media host that also carries the entry and TURN. The earlier same-machine tests used certificates signed by the entry (ADR 0015).

## Context

The components of ADR 0001 run on hardware that the operator controls. PostgreSQL and Redis must not be reachable from the public network. The media server needs a public address. Some operators have one host with a public address; other operators keep the data on a host that cannot receive inbound connections and add a second host that has a public address. The recorder needs several CPU cores while it records, which a small public host may not have.

Step 0 measured the following (`docs/step-0/README.md`, tests 3, 5 and 6):

- The LiveKit server needs Redis for running calls, with one node too. Calls stopped 6 seconds after Redis stopped, and new joins and the server API failed.
- A recorder on a host that cannot receive connections recorded calls of a LiveKit server on another host. It reached the LiveKit server on its public address and reached Redis through a private link.
- With the private link down for 30 seconds, calls and recordings continued and every recording file was complete.
- A port that Docker publishes was reachable before any rule for it existed in the host firewall.
- A plain restart of the recorder container ended the recorder after about one second and lost every recording in progress.

The deployed two-host form on 2026-10-16 gave a different recording result from the step-0 link test: subscriber media continued during a 30-second stopped link, but the recording ended `partly_failed`, with six validated files and three failed segments (two `recorder_lost`, one `recorder_error`). Recorder logs showed signal-connection and deadline failures, without a recorder restart or an out-of-memory event. The step-0 result does not establish uninterrupted recording for this deployment.

## Decision

Containers:

- The system is deployed with Docker Compose, with a separate container per component.
- PostgreSQL runs on an internal Docker network with no published port. Only the server container is on that network.

Redis:

- Redis is part of the call path. It runs on the host of the LiveKit server, with a restart policy and a health check.
- Redis requires a password. Its port is not published, or is published on the loopback address only. A rule of the host firewall does not protect a port that Docker publishes.
- In the two-host form Redis is also reachable on the address of the private link, through a forwarder of the link container, and on no other address outside its host.

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
- The firewall opens the media ports for IPv4 and IPv6. The metrics port of LiveKit is not configured; when it is, it listens on all addresses with host networking, and the firewall keeps it closed.
- The Compose files, their networks, users, limits and secrets are in ADR 0015, and the images in ADR 0014.

## Consequences

- In the two-host form the private link is required from the first version, because the recorder uses it for Redis.
- In the two-host form the server is unavailable while the network connection of the data host is down.
- A 30-second loss of the private link can leave a recording partly failed while calls continue. The recorder's public media path does not remove its Redis control dependency on the link. The server preserves valid files, declares lost jobs and starts replacements when the call still runs (ADR 0601, ADR 0604, ADR 0606); it does not guarantee uninterrupted recording through link loss. During the step-0 loss a stop request was not delivered, and a state change reached the LiveKit server 16 seconds after the link returned. A loss longer than 30 seconds was not tested.
- A stop of Redis ends every call within seconds. After Redis returns, a client whose SDK has ended its reconnection attempts is no longer in the call, so the client joins again by itself.
- The host that runs the recorder needs CPU cores that no other component takes while a recording runs. The measured use for one recording is in ADR 0602. The transcription worker on the same host must not take those cores during a recording.
- The capacity of a media host depends on its hardware and is measured per deployment with test participants. The load points that step 0 measured are in `docs/step-0/README.md`, test 6. A recording did not raise the CPU use of the media server by a measurable amount. The CPU use follows the number of packets and of connections, so many participants who send audio at the same time cost more than a few video streams of the same total bitrate.
- Whether a call admission check runs while the private link is down depends on the path of the server's requests to the LiveKit API (ADR 0506). Requests to the LiveKit API and signalling through the link were not tested.
- Redis 7.2 is the last version line under the BSD licence. The Redis version, or a compatible server, is chosen with the licence check of ADR 0801.
