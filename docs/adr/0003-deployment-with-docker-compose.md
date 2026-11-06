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

From 2026-11-02 to 2026-11-03 the private link of a deployed two-host form passed no packets for 33 hours, and the server was unreachable. The health check of the link container marked it unhealthy, but the restart policy of Docker starts a container again only after its process ends, and the process of the link ended only when one of its programs ended. A manual restart of the link container restored the link. The cause of the loss was not found.

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
- Each link container ends its process when the latest WireGuard handshake, or its own start before the first handshake, is older than a bound, and the restart policy of Docker starts it again. A new start replaces the WireGuard process and its UDP socket, the interface and its address, every forwarder, and the network namespace of the container. On the data host the new socket has a new source port, so state that the network between the hosts, or a host, keeps for the old port no longer applies. `wireguard-go` attempts a handshake for 90 seconds and then waits until it has a packet to send; a new start sends the first keepalive again.
- The bound is `JUNTO_LINK_RESTART_AFTER`, at least 180 seconds. WireGuard renews the keys of a session about every 2 minutes while packets pass, and rejects them 180 seconds after their handshake; a handshake older than 180 seconds means that no packet passes, and a shorter bound would end a working link. The default is 300 seconds on the data host: a loss of up to about 2 minutes, such as a restart of the public host, ends without a restart and the TCP connections of the forwarders stay, and a longer loss ends the link after at most 5 minutes plus a start. The default on the public host is 900 seconds. The data host opens the link, and its start renews the path; the public host only waits for it. A start of the public link while the data host sends removes its published port for about a second. With a bound three times as long, the public host starts again only when the starts of the data host did not restore the link.
- The handshake is the one of the last read of `wg` that answered with a number. A read in which `wg` fails or prints no number, such as a control socket that does not answer once, leaves it as it was: it is no handshake that is missing, and ends no link that works.
- The shell of the link (BusyBox ash) ends at a fork that fails, and Docker then starts the link again, which ends every forwarded connection. A forwarder (`socat`) forks one process per connection, so the forwarders together take at most the limit of processes of the container (`pids_limit`, 2048) less 256 (`max-children`); the 256 are for the threads of `wireguard-go`, the loop, the health check and the notices. An entry of `LINK_FORWARDS` can state its number of connections after a `/`; the entries without one share the rest in equal parts. The compose files state 256 connections for Redis and 128 for the API of the media server, and the forwarder of the server takes the rest, 1408 connections (ADR 0015). Redis carries the connections of the recorder and of each of its jobs, up to 48 track jobs by its processor budget; the API of the media server carries the requests of the server. A forwarder at its bound accepts no new connection until one of its connections ends: a new connection waits in the queue of the listening socket and then times out, and the log of the link states nothing of it. Bounds that the limit does not hold end the start with a message, and so does a container whose limit can be read neither from cgroup v2 nor from the pids controller of cgroup v1; a container without a limit (`max`) gives each forwarder without a stated number 1000 connections. A pass of the loop forks only while 32 processes of the container are free, which it reads from the files of its cgroup with a builtin of the shell; otherwise it waits one second without a fork (a read with a time limit) and keeps its state, and states this once in its log. The test `deploy/tests/link-entrypoint.sh` checks the bound of three forwarders against the `pids_limit` of the compose files, and lowers the limit of a running link to 2: the link keeps running without a read of `wg` and reads again when the limit is raised. The script before this rule ended there with "can't fork".
- `JUNTO_LINK_NOTICE_URL`, when set, receives an HTTP `POST` with a plain text body, which ntfy accepts, when the link of a host goes down (the first time its bound is passed) and when it is up again. A link that stays down starts again every bound and sends no further notice. A loss ends at a renewal of the keys: a handshake newer than the first handshake of the process, so about 2 minutes after the first. A path that passes one handshake after each start and then no packet stays down and sends no notice per start. The state of a loss (its time, the number of starts, and whether its notice was delivered) is the file `down` in the volume of the keys of the link, because a start of the container keeps its volumes and not its process. The process writes it before it ends and sends no notice then, so a notice that waits for its address delays no restart, and a stop of the container during a notice loses no state; the next start sends the notice in its first second. A notice that was not delivered is sent again every 60 seconds until it is delivered. The client is curl, which checks the certificate of an HTTPS address and reads the address and the text from its configuration on standard input: for ntfy the address is the secret of the topic, and the argument list of a process is visible in the process list of the host.
- Each host sends its notices over its own connection to the Internet, not over the link, so the notice of the data host arrives when only the link or the public host fails. When the data host has lost its connection, only the notice of the public host arrives, after the bound of the public host.

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
- A link that the other host does not answer starts again every 300 seconds on the data host and every 900 seconds on the public host, without an end, and writes a few log lines at each start; the logs of every container are limited to five files of `JUNTO_LOG_MAX_SIZE` (`deploy/base.yaml`). A loss that a new process, socket and network namespace do not end, such as a failure of the network between the hosts, stays until that network returns; the notices state it, and the link is up again by itself after the first handshake.
- The health check still marks a link unhealthy after 180 seconds without a handshake, before the bound ends it.
- `deploy/tests/link-entrypoint.sh` runs the start script in the link image with stand-ins for `wireguard-go`, `wg`, `ip`, `date` and `curl`: a read of `wg` that fails while a handshake exists ends no link (the script before this rule ended it), a restart writes the loss and sends no notice, the next start sends it, the first handshake of a start does not end the loss and a renewal does, and the address is not in the arguments of curl.
- `deploy/tests/link-recovery.sh` starts only the two link containers on one machine, with a receiver of notices. It keeps both ends up for longer than both bounds and checks that neither starts again. It stops the public link, and checks that the data link starts again twice and sends one notice for the loss and one for its return; the receiver refuses the first notice with 503, which must arrive later once, and every notice must be plain text; then it drops the packets of the public link to the data host with a route in its namespace, and checks that the public link starts again without the route and sends its two notices. It also checks that a bound below 180 seconds is refused. The link of the commit before this decision fails it: after 480 seconds without the public link, the data link had not started again.
- A 30-second loss of the private link can leave a recording partly failed while calls continue. The recorder's public media path does not remove its Redis control dependency on the link. The server preserves valid files, declares lost jobs and starts replacements when the call still runs (ADR 0601, ADR 0604, ADR 0606); it does not guarantee uninterrupted recording through link loss. During the step-0 loss a stop request was not delivered, and a state change reached the LiveKit server 16 seconds after the link returned. A loss longer than 30 seconds was not tested.
- A stop of Redis ends every call within seconds. After Redis returns, a client whose SDK has ended its reconnection attempts is no longer in the call, so the client joins again by itself.
- The host that runs the recorder needs CPU cores that no other component takes while a recording runs. The measured use for one recording is in ADR 0602. The transcription worker on the same host must not take those cores during a recording.
- The capacity of a media host depends on its hardware and is measured per deployment with test participants. The load points that step 0 measured are in `docs/step-0/README.md`, test 6. A recording did not raise the CPU use of the media server by a measurable amount. The CPU use follows the number of packets and of connections, so many participants who send audio at the same time cost more than a few video streams of the same total bitrate.
- Whether a call admission check runs while the private link is down depends on the path of the server's requests to the LiveKit API (ADR 0506). Requests to the LiveKit API and signalling through the link were not tested.
- Redis 7.2 is the last version line under the BSD licence. The Redis version, or a compatible server, is chosen with the licence check of ADR 0801.
