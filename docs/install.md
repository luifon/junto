# Installing a Junto server

This document describes how a person with a Linux host and Docker installs a complete Junto server from this repository, and how to back it up, upgrade it and remove it. The decisions behind it are in ADR 0003 (the two forms), ADR 0004 (names and network paths), ADR 0005 (backups and upgrades), ADR 0014 (images), ADR 0015 (the Compose files), ADR 0016 (the hand-over to the transcription worker) and ADR 0020 (the recorder container).

The project publishes no built image. Every image is built on the host from this repository, or pulled from its publisher by a digest that the repository states.

## What runs

| Container | What it is | Image |
|---|---|---|
| `server` | The Junto server, with `ffmpeg`, `ffprobe` and the recording layout page | built from `deploy/images/server/` |
| `postgres` | PostgreSQL 17 | pulled |
| `livekit` | The media server, LiveKit 1.13.9 | pulled |
| `redis` | Redis 7.2, which the media server and the recorder need | pulled |
| `recorder` | The recorder, LiveKit Egress 1.15.0 | pulled |
| `worker` | The transcription worker, and the loop that takes its jobs | built from `workers/transcription/` |
| `entry` | The entry on port 443: TLS for three server names | pulled |
| `link` | One end of the private link, in the two-host form only | built from `deploy/images/link/` |
| `init` | Sets owner and mode of the volume directories at each start, then exits | the server image |

There are two forms (ADR 0003), each with its own Compose file in `deploy/`:

1. **One host** with a public address: `one-host.yaml`. All containers run on that host.
2. **Two hosts**: `two-hosts-data.yaml` on the data host (server, PostgreSQL, files, recorder, worker) and `two-hosts-public.yaml` on the public host (media server, Redis, entry). The data host needs no inbound connection; its link container opens a WireGuard link to the public host.

A third file, `local.yaml`, runs everything on one machine without DNS names and TLS, for a trial (see "Trial on one machine").

The `docker compose` commands of this document run in the directory `deploy/`, where Compose finds `.env`. The commands that start with `deploy/` run in the root of the repository.

## Requirements

- A Linux host on x86-64 or arm64 with Docker Engine and the Compose plugin. The files were run with Docker Engine 29.2 and Compose 5.0.
- `git` and `openssl` on the host.
- A domain with three names that point to the public address of the host (of the public host in the two-host form), for example `chat.example.test`, `livekit.example.test` and `turn.example.test`. Each needs an A record, and an AAAA record when the host has IPv6.
- The public IPv4 address of that host.
- Memory: the default limits of all containers add up to 14.4 GB. Measured use: the recorder 1.6 GiB for one recording of 1080p, the worker 3.2 GB for a transcription on a CPU, the media server up to 0.5 GiB. Each limit is a setting in `deploy/.env`.
- CPU: one recording used 2.5 to 3.9 cores of the recorder. The server records two calls at a time. A transcription does not start while a call is recorded.
- Disk: the server keeps 5 GiB free on the volume of the recordings and reserves 8 GiB for each recording, so a recording starts only with 13 GiB free. The transcription models need 1.7 GB.

### Ports

Open these ports in the firewall of the host with the public address, for IPv4 and for IPv6:

| Port | Used for |
|---|---|
| 443/tcp | The server (HTTPS), the signalling of the media server, and TURN over TLS, selected by server name |
| 80/tcp | Certificate requests of the entry (ACME), and a redirect to HTTPS |
| 7882/udp | Call media |
| 7881/tcp | Call media when a network blocks UDP |
| 3478/udp | TURN |
| 51820/udp | The private link, on the public host of the two-host form only |

Keep every other port closed. The media server and the entry use the network of the host, so a rule of the host firewall applies to them. Two ports of the media server listen on every address of the host and must stay closed in the firewall: 5349/tcp (TURN without TLS, which only the entry uses) and the UDP ports that TURN allocates. The API port 7880 of the media server listens on the loopback address and on the address of the container network only.

A port that Docker publishes is reachable even when the host firewall has no rule for it (ADR 0003). In these files only the link publishes a port, 51820/udp, which is meant to be public. PostgreSQL, Redis and the server publish no port.

### Kernel setting of the media host

The media server asks for UDP buffers of 5,000,000 bytes (ADR 0003). On the host that runs `livekit`:

```sh
printf 'net.core.rmem_max=5000000\nnet.core.wmem_max=5000000\n' | sudo tee /etc/sysctl.d/90-junto.conf
sudo sysctl --system
```

## One host: from an empty host to the first login

1. Get the repository at the release that you install, and go to `deploy/`:

   ```sh
   git clone <address of the repository> junto && cd junto/deploy
   ```

2. Create the settings file with new secrets. `.env` is not tracked by git.

   ```sh
   umask 077
   cp env.example .env
   ./generate-secrets.sh >> .env
   ```

   Edit `.env`: set `JUNTO_SERVER_HOST`, `JUNTO_LIVEKIT_HOST`, `JUNTO_TURN_HOST`, `JUNTO_PUBLIC_IP`, `JUNTO_ACME_EMAIL` and `JUNTO_MODELS_DIR`. `env.example` explains every setting. No secret has a default: Compose stops with the name of the variable when one is missing. A secret consists of letters and digits; PostgreSQL, Redis and the media server stop with a message when one has another character.

3. Install the transcription models (ADR 0612). Build the worker image, download the files as your own user, and verify them without a network:

   ```sh
   docker compose -f one-host.yaml build worker
   sudo install -d -o "$(id -u)" -g "$(id -g)" -m 0755 /srv/junto/models
   docker run --rm --user "$(id -u):$(id -g)" -v /srv/junto/models:/models junto-worker models fetch --dir /models
   docker run --rm --network none -v /srv/junto/models:/models:ro junto-worker models verify --dir /models
   ```

   The download is 1.7 GB. The worker's user (10001) reads the directory and cannot write to it. To run without transcripts, set `JUNTO_TRANSCRIBE_COMMAND=` (empty) in `.env`, point `JUNTO_MODELS_DIR` to any empty directory, and add `--scale worker=0` to the `up` command.

4. Apply the kernel setting and the firewall rules above. Check that the three DNS names resolve to the host.

5. Build and start:

   ```sh
   docker compose -f one-host.yaml up -d --build
   docker compose -f one-host.yaml ps
   ```

   The first build compiles the server, FFmpeg and the worker's FFmpeg and PyAV, and takes several minutes. The entry requests the three certificates at its first start; this needs the ports 80 and 443 and the DNS names.

6. Read the one-time owner code. The server prints it at each start while it has no owner:

   ```sh
   docker compose -f one-host.yaml logs --no-log-prefix server | grep -A 3 "no owner"
   ```

7. Become the owner with a client. In the desktop client, add the server `https://chat.example.test` and enter the owner code. With the command-line client of this repository (`cargo build -p junto-client-core --bin junto-cli`):

   ```sh
   junto-cli add https://chat.example.test
   junto-cli claim --name "Your name"      # reads the owner code from standard input
   junto-cli status
   ```

   The first device that presents the code becomes the server owner (ADR 0110). Later logins use the device key: `junto-cli login`.

8. Check the health route. It answers 200 while the server reaches its database; the body states whether it reaches the media server (ADR 0012):

   ```sh
   curl https://chat.example.test/api/v1/health
   ```

## Two hosts

Both hosts get the repository and a `deploy/.env`. The LiveKit key, the LiveKit secret and the Redis password are the same on both hosts; the PostgreSQL password is needed on the data host only.

1. On the data host: `umask 077 && cp env.example .env && ./generate-secrets.sh >> .env`. Copy the lines `JUNTO_LIVEKIT_API_KEY`, `JUNTO_LIVEKIT_API_SECRET` and `JUNTO_REDIS_PASSWORD` to the `.env` of the public host over a channel that you trust.
2. On both hosts: set the names and `JUNTO_PUBLIC_IP` in `.env`. The public host needs `JUNTO_TURN_HOST` and `JUNTO_ACME_EMAIL`; the data host needs `JUNTO_MODELS_DIR`.
3. Create the key of each end of the link. The private key stays in a volume of its host; the command prints the public key.

   ```sh
   docker compose -f two-hosts-public.yaml run --rm link genkey    # on the public host
   docker compose -f two-hosts-data.yaml run --rm link genkey      # on the data host
   ```

   Put the key that the data host printed into the `.env` of the public host as `JUNTO_LINK_DATA_PUBLIC_KEY`, and the key that the public host printed into the `.env` of the data host as `JUNTO_LINK_PUBLIC_PUBLIC_KEY`. A link container does not start while its line is missing, and its log names the setting.
4. On the public host: the kernel setting, the firewall rules including 51820/udp, then `docker compose -f two-hosts-public.yaml up -d --build`.
5. On the data host: the transcription models as in step 3 of the one-host form (the image is `junto-data-worker`), then `docker compose -f two-hosts-data.yaml up -d --build`. The data host needs outbound connections to the public host: UDP to port 51820, and the media ports, because the recorder connects to the media server on its public address.
6. Continue with step 6 of the one-host form, with `two-hosts-data.yaml`.

The server is unavailable while the link is down, because the entry reaches the server through it. Calls and recordings continue during a short loss of the link (ADR 0003).

## TLS

The entry (Caddy with its layer-4 module) obtains one certificate for each of the three names from a public certificate authority with ACME and renews them by itself. It keeps the certificates and its ACME account key in the volume `entry-data`. `JUNTO_ACME_EMAIL` is the contact address that the authority gets.

A connection with the TURN name is ended at the entry and passed to the media server as plain TCP on the loopback address. The other two names are HTTP sites that the entry forwards. The entry does not forward `/internal/` and `/recording-layout/` of the server; only the media server and the recorder call them, from inside.

The server itself has no TLS. It is reachable only through the entry, and it accepts a login only for the origin `https://<JUNTO_SERVER_HOST>` (ADR 0117).

## Trial on one machine

`local.yaml` starts every container except the entry on one machine, with all ports on the loopback address. It needs only the four secrets and `JUNTO_MODELS_DIR` in `.env`.

```sh
docker compose -f local.yaml up -d --build
docker compose -f local.yaml logs --no-log-prefix server | grep -A 3 "no owner"
junto-cli add http://127.0.0.1:8080
```

A client on another machine cannot log in to this form: the server accepts `http` origins for a loopback address only. `deploy/tests/smoke.sh` starts this form with new secrets, checks it and removes it. `deploy/tests/recording.sh` records a call in this form with test participants and checks the video, the audio files, the transcript and the downloads (ADR 0910).

## Recordings

The recorder loads the layout page of a recording from the server, at `http://server:8080/recording-layout/` on the container network. The page is built into the server image, and the image sets `JUNTO_RECORDER_LAYOUT_DIR` to its directory (ADR 0014, ADR 0615). The entry does not forward that path. Nothing is to be installed for it.

- The recorder container holds the key and the secret of the media server and the password of Redis. It runs without a capability, and it is not on the network of the database (ADR 0020).
- A recording starts only with 13 GiB of free space on the volume of the recordings: 8 GiB for the recording above a reserve of 5 GiB (ADR 0605). A start with less is answered with `storage_low`. The health route states `storage_low` only at 5 GiB or less.
- The files of a recording are below the volume `recordings`: `<recording>/<segment>.mp4` for the video and `<recording>/<segment>.ogg` for each audio track. A removal of the recording through the API deletes its directory.
- When the recorder container ends during a recording, its restart rule starts it again, its start script saves the partial audio files, and the server continues the recording in new files. The recording then has the state `partly_failed`, with a video after the loss and, when the recorder had written a piece of the video before the loss, a video before it (ADR 0601).
- When the recorder stayed stopped until a recording that it had lost was removed, its next start saves the partial audio of that recording to `rescued/<job>/` on the volume of the recordings, and nothing deletes it (ADR 0606). After such a case, and when no recording runs and every recording has a final state, delete the directories below `rescued/`: `docker compose -f <form file> exec server find /var/lib/junto/recordings/rescued -mindepth 1 -delete`.

## Logs and health

- `docker compose -f <form file> ps` shows the state and the health of each container. Every container except `worker` and `init` has a health check.
- `docker compose -f <form file> logs <container>` shows the log. Docker keeps five files of 10 MB per container (`JUNTO_LOG_MAX_SIZE`).
- The server writes structured log records to standard error and the owner code to standard output.
- Keep the log level of the recorder above `debug`: its debug log contains the room token of the recorder (ADR 0003).

## Backup

The server has no backup command yet (ADR 0005 states one). Until it has, `deploy/backup.sh` makes a consistent copy with the server stopped:

```sh
deploy/backup.sh one-host.yaml /srv/junto-backups/2026-10-11
```

It runs on the host that has the data (`one-host.yaml` or `two-hosts-data.yaml`). It stops `server` and `worker`, so that no recording can start. When a recording still runs, it starts the two again and ends with the code 3; end the recording and repeat. Otherwise it writes the database (`database.dump`, the custom format of `pg_dump`), the attachments and the recordings (`files.tar`), a copy of `.env` (`env`) and their checksums, and starts what it stopped. A container that was stopped before the backup stays stopped. Nobody can use the server during that time.

The directory of the backup contains every secret of the deployment. Store it accordingly, and store a copy on another machine.

Not in the backup: the certificates of the entry and the keys of the link. The entry requests new certificates, and new link keys are created as in the installation.

## Restore

On a host with the repository at the same release, or a newer one, as the backup:

```sh
cp /srv/junto-backups/2026-10-11/env deploy/.env
deploy/restore.sh one-host.yaml /srv/junto-backups/2026-10-11
(cd deploy && docker compose -f one-host.yaml up -d)
```

`restore.sh` verifies the checksums, refuses an archive that `backup.sh` did not write, and stops `server`, `worker` and `recorder`. It loads the dump into a new database and unpacks the archive into a staging directory of each volume; when one of the two fails, the database and the files of the host are as they were. Then the staged copies replace the data, the database user gets the password of `.env`, and `junto-server new-event-epoch` makes every client load a new snapshot (ADR 0010). The volumes need free space for a second copy of the files during the restore. When the script is interrupted during that replacement, run it again.

A server that is newer than the backup applies its migrations at the start. A server that is older than the database does not start (ADR 0005).

## Upgrade

The server is never updated automatically (ADR 0005). An upgrade is a new checkout of the repository and a new build.

1. Stop the server, so that no recording can start, and check that none runs. The command ends with the code 3 while one does; then start the server again (`docker compose -f one-host.yaml start server`), end the recording and repeat. Do not stop, restart or replace the recorder during a recording: the recording is lost (ADR 0601).

   ```sh
   docker compose -f one-host.yaml stop server
   docker compose -f one-host.yaml run --rm --no-deps server check-upgrade
   ```

2. Make a backup with `deploy/backup.sh`. The server stays stopped. Migrations go forward only, and the server makes no copy of the database before a migration, so this backup is the only way back.
3. Get the new release and compare `env.example` with your `.env` for new settings.

   ```sh
   git fetch && git checkout <new release>
   ```

4. Build and start. The server applies its migrations at the start.

   ```sh
   docker compose -f one-host.yaml up -d --build
   ```

The server is unavailable from step 1 until the build of step 4 has ended. To shorten that time, build the new images in a second checkout of the repository before step 1 (`docker compose -f one-host.yaml build`); the images get the same names, and step 4 then starts them without a build.

In the two-host form, upgrade the public host first when the release changes the media server or Redis; calls end while those two restart.

To go back to the earlier release: check out that release, restore the backup of step 2, and start.

## A changed secret

- `JUNTO_POSTGRES_PASSWORD` is stored by PostgreSQL at its first start. When the line in `.env` no longer matches, the server does not start and its log states `password authentication failed for user "junto"`. To change the password, change it in the database first, then in `.env`, then run `up -d`. `psql` asks for the new value, so that it is not part of a command line: `docker compose -f <form file> exec postgres psql -U junto -c '\password junto'`. Use letters and digits only.
- `JUNTO_LIVEKIT_API_KEY`, `JUNTO_LIVEKIT_API_SECRET` and `JUNTO_REDIS_PASSWORD` are read at each start. Change them in `.env` (on both hosts in the two-host form) and run `up -d`. Calls end when the media server restarts.

## Removal

```sh
docker compose -f one-host.yaml down          # stops and removes the containers; the data stays
docker compose -f one-host.yaml down -v       # also deletes every volume: database, files, certificates
docker image rm junto-server junto-init junto-worker
```

Then remove the model directory, `/etc/sysctl.d/90-junto.conf`, the firewall rules and the DNS records. `down -v` cannot be undone.

## What was not run

- The entry was tested with certificates that it signed itself (`deploy/tests/entry.sh`). A certificate request to a public authority, and a call over TURN on port 443, were not run (ADR 0004).
- `one-host.yaml` and the two files of the two-host form were started on one machine, with certificates that the entry signed itself (`deploy/tests/one-host.sh`, `deploy/tests/two-hosts.sh`). They were not started on a host with a public address, and no call ran through them.
- `worker-gpu.yaml` was checked as a rendered configuration only.
- The address that the server sees for a request of the entry was not observed on a Linux host. `JUNTO_TRUSTED_PROXIES` names the address of the host in the container network.
- A recording through the recorder was made in the local form only (`deploy/tests/recording.sh`, ADR 0910), on one machine with Docker Desktop on arm64. In the one-host form and in the two-host form the recorder reaches the media server on its public name, and no recording was made there.
- The recorder ran without a capability in the local form. It was not run on a Linux host with another seccomp or AppArmor profile (ADR 0020).
- In the local form, `rtc.stun_servers` stopped the recorder's requests to public STUN servers of third parties. The one-host form names its TURN port in the same setting; it was not observed there (ADR 0007).
