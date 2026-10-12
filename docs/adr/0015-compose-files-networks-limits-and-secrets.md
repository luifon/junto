# 0015. Compose files: services, networks, limits and secrets

## Status

Accepted

Open: `one-host.yaml` and the two files of the two-host form were started on one machine with certificates that the entry signed itself, and not on a host with a public address. The address that the server sees for the entry, a call, and a recording through the recorder were not observed in these forms. In the local form a call was recorded with the recorder and transcribed by the worker container (ADR 0910).

## Context

ADR 0003 decides two forms with a Compose file each, and which component runs on which host. ADR 0004 decides three server names on port 443. Neither states the files, the networks, the users, the limits or how a secret reaches a container. The server trusts a forwarded client address only from the IP addresses in `JUNTO_TRUSTED_PROXIES`, reaches the API of the media server over plain HTTP on a private network (ADR 0508), and has no backup command yet (ADR 0005). Step 0 ran the media server with the network of the host, where it gave IPv6 media addresses to clients, and found that a port that Docker publishes is reachable without a firewall rule. The recorder ran in step 0 with the default capabilities of Docker and `SYS_ADMIN`.

## Decision

Files:

- `deploy/base.yaml` defines each service once: image, user, capabilities, file system, limits, health check, restart rule, log limit and volumes. It reads only settings that have a default.
- A form file extends those services and adds the networks, the published ports, the addresses and every setting without a default: `one-host.yaml`, `two-hosts-data.yaml`, `two-hosts-public.yaml`, and `local.yaml`.
- `local.yaml` runs every component except the entry on one machine, without DNS names and TLS, with every port on the loopback address. It is for a trial and for the tests of the deployment, and is not a third form of ADR 0003.
- `worker-gpu.yaml` is named after a form file and gives the worker an NVIDIA device (ADR 0610).

Settings and secrets:

- Every setting is a line of `deploy/.env`, which git ignores. `deploy/env.example` explains each one and sets no secret.
- The secrets are the PostgreSQL password, the key and the secret of the media server, and the Redis password. `deploy/generate-secrets.sh` prints them with random values of letters and digits.
- No secret has a default. A form reads each of its required settings in the form `${NAME:?message}`, so Compose stops and names the variable when one is missing.
- A secret consists of letters and digits, because it is written into a database URL, a Redis configuration and YAML bodies. The containers of PostgreSQL, Redis and the media server check this before they start their program and stop with a message that names the setting.
- The public key of the other end of the link is the one setting without a default that Compose does not require: the command that creates the key of a host runs before the key of the other host exists. The link container stops with a message while it is missing.
- A secret reaches its container as an environment variable, because the server reads its settings from the environment (ADR 0008).

Users and capabilities:

- Every container drops all capabilities, cannot gain new privileges and has a read-only root file system, with these exceptions.
- `init` runs as root with `CHOWN` and `FOWNER` and without a network. It sets owner and mode of the volume directories at each start and exits. It changes the named directories only.
- `entry` runs as root with `NET_BIND_SERVICE`, for the ports 80 and 443 on the network of the host.
- `link` runs as root with `NET_ADMIN`, `SETUID` and `SETGID` in its own network namespace: it creates the tunnel interface, and its forwarders run as the user `nobody`.
- `recorder` runs as the user of its image without a capability, and has a writable root file system (ADR 0020).
- The first process of `worker` runs as root with `SETUID`, `SETGID` and `KILL` and without a network. It is the loop that takes the transcription jobs, and it runs each job as a user without root rights and without capabilities (ADR 0016).
- PostgreSQL, Redis, the media server and the server run as users without root rights from their first process.
- No container mounts the Docker socket.

Networks of the one-host form:

- PostgreSQL is on an internal network with the server only, and publishes no port.
- The media server and the entry use the network of the host, as step 0 ran the media server. The host firewall applies to them.
- The network `backend` has a fixed subnet. The server and Redis have fixed addresses in it. The media server reaches Redis and the notification route of the server at those addresses. The entry reaches the server at its fixed address.
- The API port of the media server listens on the loopback address and on the address of the host in `backend`. The server reaches it there.
- `JUNTO_TRUSTED_PROXIES` is the address of the host in `backend`, from which the entry connects.
- No container publishes a port.
- The recorder reaches the media server at its public name and Redis by its service name.

Networks of the two-host form:

- Each link container has a fixed address in `backend` and forwards TCP ports with `socat`; it forwards no IP packets (ADR 0003).
- On the public host the link listens on its link address for Redis and for the API of the media server, and forwards the HTTP port of the server to the data host. The entry and the notifications of the media server use that port.
- That HTTP port listens in a network of its own, `inbound`, whose only container is the link. The server trusts the forwarded client address of every request that arrives through the link, so only the programs on the network of the host, which are the entry and the media server, may reach the port. Redis is in `backend` and cannot.
- On the data host the link listens on its `backend` address for Redis and for the API of the media server, and on its link address for the HTTP port, which it forwards to the server. `JUNTO_TRUSTED_PROXIES` is the `backend` address of the link.
- The link of the public host publishes its UDP port. No other container of either host publishes a port.
- The data host publishes nothing and needs no inbound connection.

The entry:

- `deploy/entry/Caddyfile` is the configuration. A listener in front of the HTTP server reads the server name of each TLS connection. A connection with the TURN name is ended and passed to the TURN port of the media server as plain TCP (`turn.external_tls`). The other connections go to the HTTP server, which has one site for the server and one for the media server.
- The entry obtains the three certificates by ACME and keeps them in a volume.
- The entry does not forward `/internal/` and `/recording-layout/` (ADR 0508, ADR 0615), has no access log, removes the token from the query of an address in its error log, and has HTTP/3 off, so that UDP port 443 stays closed.
- The administration port of Caddy is off.

Media server:

- `rtc.node_ip` is the public IPv4 address of the setting `JUNTO_PUBLIC_IP`, with `use_external_ip` off.
- TURN is on: UDP port 3478, and TLS through the entry.
- `rtc.stun_servers` names the TURN port of the deployment, so that the media server gives its clients no public STUN server of a third party (ADR 0007).
- The metrics port is not configured and does not listen.
- A room stays for 60 seconds after its last person left (ADR 0020).

Recorder:

- Its start script is `deploy/images/recorder/start.sh`. It copies the partial Ogg files of an ended recorder from the temporary directory to `rescued/<job>/` below the recordings (ADR 0602), then runs the start script of the image, which deletes that directory. When a copy fails, it ends without starting the recorder, so that the only copy is not deleted.
- The temporary directory is a volume. The CPU costs are the values that step 0 measured with (ADR 0602). The log level is `info`.
- Its stop timeout is 2 minutes.

Limits and behaviour:

- Every container has a memory limit, a CPU limit and a limit of processes. The defaults are in `env.example` with the measurements that they follow.
- Every container restarts unless it was stopped, except `init`.
- Docker keeps five log files of 10 MB per container, and two of 1 MB for `init`.
- Every container has a health check, except `init` and the worker. The health check of the entry connects to port 80, because a connection without a TLS greeting on port 443 is an error in its log. The server starts after `init` ended and PostgreSQL is healthy; the media server starts after Redis is healthy.
- Redis has no volume: it holds no data that outlives a restart.

Backup, restore and upgrade:

- ADR 0021 states `deploy/backup.sh`, `deploy/restore.sh`, `deploy/upgrade-undo.sh` and the upgrade procedure. They replace the scripts of this record, which stopped the server for a backup and ran `new-event-epoch` after a restore.
- The backup does not contain the certificates of the entry and the keys of the link. Both are created again: this is narrower than ADR 0005, which names the keys.
- The server has a volume `exports` for the files of exports (ADR 0411). `init` gives it the owner and the mode of the volume of the attachments.

Tests:

- `deploy/tests/compose-config.sh` renders each form with exactly the settings that it needs, removes each setting in turn, and checks the rendered configuration against stated rules: published ports and their addresses, pinned images, users, capabilities, limits, log limits, networks, the directories of the server that are volumes, and that `init` prepares every volume of a container without root rights.
- `deploy/tests/entry.sh` runs the entry with certificates that it signs itself and three stand-in programs.
- `deploy/tests/smoke.sh` starts the local form, waits for the health of every container, and removes it.
- `deploy/tests/backup-restore.sh` makes a backup of the local form while content is written and deleted, runs restores that are refused, and restores the backup into a new, empty deployment with other secrets, with an interruption (ADR 0902, ADR 0911).
- `deploy/tests/upgrade.sh` upgrades the local form from an earlier release and goes back (ADR 0911).
- `deploy/tests/recorder-start.sh` runs the start script of the recorder with a stand-in for the start script of the image.
- `deploy/tests/one-host.sh` starts the one-host form on one machine, with certificates that the entry signs itself, and sends requests through the entry to the server and to the media server.
- `deploy/tests/two-hosts.sh` starts the two files of the two-host form on one machine as two projects with separate networks, and checks the handshake of the link, a request from the entry to the server through the link, the server's path to the API of the media server through the link, and the recorder's path to Redis.
- `deploy/tests/recording.sh` records a call through the local form with the recorder, the layout page, the worker and test participants (ADR 0910).

## Consequences

- An operator reads and edits one settings file. The secrets are in that file and in the environment of the containers, readable for every user who can use Docker on the host.
- A value of a secret that is not letters and digits can break the database URL or a YAML configuration. The generator writes letters and digits only.
- The PostgreSQL password is stored at the first start. A later change of the line in `.env` alone makes the server fail at its start with the message of PostgreSQL.
- The fixed subnet of `backend` can collide with a network of the host. Five settings move it.
- The port for TURN without TLS (5349) and the relay ports of TURN listen on every address of the host. The install document tells the administrator to keep them closed in the firewall.
- The public address also needs port 80, for the certificate requests (ADR 0004).
- In the two-host form every connection of a client to the server passes two `socat` processes. Each connection is one process of each link container; the limit of processes of the link is 2048.
- The recorder container has a writable root file system, and its browser runs without its sandbox (ADR 0020).
- An upgrade stops the server from the check for a recording until the new version has started.
- A recorder whose partial files cannot be copied, for example on a full disk, does not start until the cause is removed. Its health check fails during that time.
- The limits of a transcription job are settings of the worker container. A line of `.env` reaches the worker; without a line the worker uses its default.
- A rule that the rendered configuration breaks fails CI, for example a new published port.
