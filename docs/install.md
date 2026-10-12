# Installing a Junto server

This document describes how a person with a Linux host and Docker installs a complete Junto server from this repository, and how to back it up, upgrade it and remove it. The decisions behind it are in ADR 0003 (the two forms), ADR 0004 (names and network paths), ADR 0005 (backups and upgrades), ADR 0014 (images), ADR 0015 (the Compose files), ADR 0016 (the hand-over to the transcription worker), ADR 0018 (what the server does for a backup and before a migration), ADR 0020 (the recorder container), ADR 0021 (backup, restore and upgrade in the deployment) and ADR 0022 (the database role of the server).

The project publishes no built image. Every image is built on the host from this repository, or pulled from its publisher by a digest that the repository states.

## What runs

| Container | What it is | Image |
|---|---|---|
| `server` | The Junto server, with `ffmpeg`, `ffprobe` and the recording layout page | built from `deploy/images/server/` |
| `postgres` | PostgreSQL 17, with the access rules and the role of the server from `deploy/images/postgres/` | pulled |
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
- Disk: the server keeps 5 GiB free on the volume of the recordings and reserves 8 GiB for each recording, so a recording starts only with 13 GiB free. The transcription models need 1.7 GB. An export, an upgrade and a restore need further space for a time (see "Exports", "Upgrade" and "Restore").

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
- When the recorder stayed stopped until a recording that it had lost was removed, its next start saves the partial audio of that recording to `rescued/<job>/` on the volume of the recordings. No recording uses that directory. The server deletes it at a maintenance pass when its last change is more than 24 hours ago (ADR 0606). A pass runs every minute and examines 50 entries below `rescued/`, and it deletes nothing while a backup holds deletions back.

## Logs and health

- `docker compose -f <form file> ps` shows the state and the health of each container. Every container except `worker` and `init` has a health check.
- The health check of the server container is the command `junto-server health`. It asks the health route of the server in the same container (ADR 0012).
- `docker compose -f <form file> logs <container>` shows the log. Docker keeps five files of 10 MB per container (`JUNTO_LOG_MAX_SIZE`).
- The server writes structured log records to standard error and the owner code to standard output.
- Keep the log level of the recorder above `debug`: its debug log contains the room token of the recorder (ADR 0003).

## Exports

A person with the permission exports a channel or a direct conversation as one ZIP file (ADR 0411). The server writes the files below the volume `exports` (`JUNTO_EXPORTS_DIR` in the container, set by the Compose files). Nothing is to be installed for it.

- An export is kept for 7 days. An account has at most 3 exports that wait or are written, and 10 stored exports. The server writes one export at a time.
- The storage guard (ADR 0605) applies to exports: a request is answered with `storage_low` while the free space of the volume is at or below 5 GiB, and an export reserves the bytes of its file before it writes them. While it is written, an export needs up to twice its size.
- The reservations of recordings, uploads and exports are one sum. The volumes of a host are on one file system unless the host places them elsewhere, so the three share its free space, and the guard counts them together.
- Exports are not in a backup, and a restore deletes them: they are copies of content that the backup holds, and a person asks for an export again.

## The database users

PostgreSQL has two users in a deployment (ADR 0022):

- `junto_server` is the role of the server. It may log in and create a database, and has no other attribute. It owns the database `junto` and every table in it. The server needs both rights for the copy before a migration (ADR 0018). Its password is `JUNTO_POSTGRES_PASSWORD`.
- `junto` is the first user of the PostgreSQL container and has every right. It connects through the socket inside that container only: the access rules in `deploy/images/postgres/pg_hba.conf` let no other role than `junto_server` connect over the network. The scripts of `deploy/` and the `psql` commands of this document use it, as `docker compose -f <form file> exec postgres psql -U junto -d postgres`. They name the database `postgres`: the role of the server owns the database `junto` and can change what a name means in it, so the first user reads nothing there except through `deploy/images/postgres/server-role.sql`, which reads names of the system schema only.

The PostgreSQL image creates the role when it creates the data directory, with `deploy/images/postgres/server-role.sql`. A deployment that an earlier release created does not have it; see "The database user of an earlier release".

## Backup

`deploy/backup.sh` makes a backup while the server runs:

```sh
deploy/backup.sh one-host.yaml /srv/junto-backups/2026-10-11
```

It runs on the host that has the data (`one-host.yaml` or `two-hosts-data.yaml`), and it needs the container `postgres` to run. Its steps (ADR 0018, ADR 0021):

1. `junto-server schema-version` states the schema of the server binary, which the script writes into the file `schema`. `junto-server unnamed-files --remove` deletes the directories that no row of the database names and that were last changed more than 24 hours ago (see "Directories that the database does not name").
2. `junto-server backup-begin`: from here on the server deletes no directory of an attachment or of a recording.
3. `pg_dump` writes the database (`database.dump`, the custom format).
4. `junto-server backup-files` lists the directories that this copy of the database names. The script writes the ones that exist into `files.tar`, and checks that the archive holds each of them.
5. `junto-server backup-end`: the server deletes files again. The command fails when the backup ended by itself during the copy, and the script then writes no backup.
6. The script adds a copy of `.env` (`env`), the file `left-out` and the checksums (`SHA256SUMS`).

The commands of the server binary run in the container of the server while it runs, and in a container of the server image while it is stopped. The script makes no backup when that image has another schema than the database, which is the case when the image was built from another release while the server was stopped: the commands of a release are written for its own schema. Build the image in the checkout of the release that ran.

Nobody is interrupted. A file that a person deletes during the backup stays on the disk until step 5. A backup that ends with a failure removes what it wrote and tells the server that it ended. A backup that is ended by force holds deletions back for 24 hours at most. The server ends a backup by itself after 24 hours; a copy that takes longer fails.

`left-out` names what the list of the server names and the archive does not hold, one directory per line: `skip <directory> upload_not_complete`, `skip <directory> recording_runs` and `skip <directory> files_are_processed` for an upload or a recording that had not ended, and `absent <directory>` for a directory that is not on the disk. The script prints the number of both. For a backup with every recording, wait until no recording runs. A recording that failed has no file and no line.

The exit code is 0 for a backup, and 4 for a backup that was written although the directory of a complete attachment or of a finished recording is not on the disk: `left-out` has a line `absent attachments/<directory>` or `absent recordings/<directory>` for it. That is a loss of a file on the host, not a state of the server. Every other code means that no backup was written.

`tar` can print that a file changed or went away while it was read. The server writes the parts of a running upload and the directories of a transcription run beside complete files; a complete file does not change. A backup fails when the directories hold something that is not a file or a directory, such as a link: the server writes none, and a restore refuses it.

The directory of the backup contains every secret of the deployment and is readable for its owner only. Store it accordingly, and store a copy on another machine.

Not in the backup: the exports, the certificates of the entry, the keys of the link, and the copy of the database from before a migration. The entry requests new certificates, and new link keys are created as in the installation.

## Restore

A backup is restored with the release that wrote it. On a host with the repository at that release:

```sh
cp /srv/junto-backups/2026-10-11/env deploy/.env
deploy/restore.sh one-host.yaml /srv/junto-backups/2026-10-11
(cd deploy && docker compose -f one-host.yaml up -d)
```

`restore.sh` first verifies the checksums and refuses an archive that `backup.sh` did not write: one with a member outside the two directories, or with a link. It compares the file `schema` of the backup with the answer of `junto-server schema-version` of the server image, and refuses a backup of a newer and of an earlier schema; its message names both numbers. The image is the one that the restore then works with, the image of the service `server` by its name. It is built when it does not exist; when it was built from another release than the checkout, build it again first (`docker compose -f one-host.yaml build server`). Until here no container is stopped. Then it stops `server`, `worker` and `recorder`, ends a container that an earlier run of the script left, and works in two steps (ADR 0021). It needs the image of the recorder on the host, because a container of the recorder empties its temporary directory:

1. Staging. A new database `junto_restore` is created. The role `junto_server` is created when the database server does not have it, gets the password of `.env`, and becomes the owner of that database (ADR 0022). The role loads the dump. `junto-server restored` gives that database a new epoch of the event stream, so that every client loads a new snapshot (ADR 0010), and removes its exports and its running backup, and the archive is unpacked into a staging directory of each volume. When one of these fails, the database and the files of the host are as they were; the role of the server can have the password of `.env` then.
2. Replacement. The database `junto` is deleted, the staged files take the places of the files, and `junto_restore` gets the name `junto`. What belonged to the replaced data is deleted: the exports, the transcription jobs in the spool, the partial files in the temporary directory of the recorder, and the copy of the database from before a migration.

The volumes need free space for a second copy of the files, and PostgreSQL for a second copy of the database, until step 2.

During step 2, after its first action, no database `junto` exists. When the script is interrupted there, the server does not start, and its log states `database "junto" does not exist`. Run the restore again: every run restores everything from the backup. A run that fails reads the state of the database server and states which of these cases holds.

A backup of another release than the checkout is refused: the command `restored` accepts the schema of its own release only, and a server that is older than the database does not start (ADR 0005). Check out the release of the backup, restore, and upgrade. A backup that the first release with the backup commands wrote has no file `schema`. The script then learns the schema in step 1, from `junto-server restored`, and ends there with the message of the server when the release differs; nothing is replaced.

The archive can hold a directory that its database does not name: an upload that ended, or a recording that was finished, between the steps 3 and 4 of the backup. After the replacement the script runs `junto-server unnamed-files` and prints these directories. No route returns them. See "Directories that the database does not name".

### Directories that the database does not name

A restore and the way back after an upgrade can leave a directory of an attachment, of a recording or of an export that no row of the database names. `junto-server unnamed-files` lists them, and with `--remove` deletes them (ADR 0018):

```sh
docker compose -f one-host.yaml exec server junto-server unnamed-files
docker compose -f one-host.yaml exec server junto-server unnamed-files --remove
```

- `unnamed <directory>`: no row names the directory, and its last change is more than 24 hours ago. With `--remove` the line is `removed <directory>`.
- `skip <directory> changed_recently`: no row names the directory, and it was changed in the last 24 hours. The command does not delete it. The server writes the row of an upload before it creates the directory, and the 24 hours are a second condition that does not depend on that order: the command never deletes a directory that the server is writing. A restore gives each directory the time of its last change before the backup, so the directories of a backup that is older than one day are `unnamed` at once.
- The command deletes nothing while a backup holds deletions back.

`deploy/backup.sh` runs the removal at the start of each backup and prints what was removed. A host with regular backups needs no further step.

## Upgrade

The server is never updated automatically (ADR 0005). An upgrade is a new checkout of the repository and a new build.

1. Stop the server, so that no recording can start, and check that none runs. The command ends with the code 3 while one does; then start the server again (`docker compose -f one-host.yaml start server`), end the recording and repeat. Do not stop, restart or replace the recorder during a recording: the recording is lost (ADR 0601).

   ```sh
   docker compose -f one-host.yaml stop server
   docker compose -f one-host.yaml run --rm --no-deps server check-upgrade
   ```

2. Make a backup with `deploy/backup.sh`. The server stays stopped, so the backup holds everything up to the upgrade. When the script states that directories are not in the backup, read `left-out`: for a line `skip <directory> files_are_processed`, start the server, wait until the recording is `ready`, and repeat from step 1.
3. Check the free space for the copy of step 6: the size of the database, on the file system of the volume `postgres`. The storage guard does not count this space.

   ```sh
   docker compose -f one-host.yaml exec postgres psql -U junto -d postgres -Atc "select pg_size_pretty(pg_database_size('junto'))"
   ```

4. Get the new release and compare `env.example` with your `.env` for new settings.

   ```sh
   git fetch && git checkout <new release>
   ```

5. Only when the deployment was created by a release in which the server connected as the user `junto`: run `deploy/database-user.sh one-host.yaml` (see "The database user of an earlier release").
6. Build and start. When the release has a migration, the server first copies the database to `junto_before_<number>` on the same PostgreSQL server, then applies the migrations (ADR 0018). Its log names the copy.

   ```sh
   docker compose -f one-host.yaml up -d --build
   docker compose -f one-host.yaml ps
   docker compose -f one-host.yaml logs --no-log-prefix server | grep junto_before_
   ```

The server is unavailable from step 1 until the build of step 6 has ended. To shorten that time, build the new images in a second checkout of the repository before step 1, under another project name: `docker compose -p junto-next -f one-host.yaml build`. The images then have the names `junto-next-...`, the images of the release that runs keep their names for the steps 1 and 2, and the build of step 6 takes its layers from the build cache. Remove the images `junto-next-...` afterwards.

In the two-host form, upgrade the public host first when the release changes the media server or Redis; calls end while those two restart.

The server keeps one copy. The upgrade that follows a finished one replaces it. To free the space earlier, when the new release works: `docker compose -f one-host.yaml exec postgres dropdb -U junto junto_before_<number>`.

### The database user of an earlier release

In the first releases the server connected to PostgreSQL as `junto`, the first user of the container, which has every right and owns the database and its tables. From this release on the server connects as `junto_server` (see "The database users"). PostgreSQL does not let the first user lose its rights, so the deployment gets the new role, once, in the checkout of the new release and with the server stopped:

```sh
deploy/database-user.sh one-host.yaml
```

The script starts `postgres` with the Compose file of the checkout, creates the role `junto_server` with the password of `.env`, and makes it the owner of the database `junto`, of each copy from before a migration, and of the tables, views, sequences, routines, types and schemas that `junto` owns in them. Each database changes in one transaction. A second run changes nothing. A role that the operator gave to `junto_server`, and a right that the operator granted to it, stay.

To learn whether a deployment needs the step: `docker compose -f one-host.yaml exec postgres psql -U junto -d postgres -Atc "select count(*) from pg_roles where rolname = 'junto_server'"` prints `0`.

Without the step the server of the new release does not start, and its log states `password authentication failed for user "junto_server"`. `deploy/backup.sh` states the missing role and makes no backup. `deploy/restore.sh` needs no step: it creates the role for the restored database.

The way back to an earlier release needs no step for the users. The Compose file of that release starts PostgreSQL without the access rules of this release, and its server connects as `junto`. `deploy/upgrade-undo.sh` gives the role that the server of its checkout connects as the password of `.env`, because a change of the secret in this release changes the password of `junto_server` only.

### When the server does not start after an upgrade

The container of the server starts again and again while its start fails. Stop it, then read the log:

```sh
docker compose -f one-host.yaml stop server
docker compose -f one-host.yaml logs --no-log-prefix --tail 20 server
```

- `password authentication failed for user "junto_server"`: the deployment was created by an earlier release and does not have the role of the server, or the password of `.env` is not the one of the role. See "The database user of an earlier release" and "A changed secret".
- `the copy of the database before the migration could not be made`: nothing was migrated. The log states the message of PostgreSQL after it. The copy needs that no other program is connected to the database `junto` (close every `psql` session), free disk space, and a role of the server that owns the database and may create one. `deploy/database-user.sh` gives the role `junto_server` both again. `JUNTO_MIGRATION_COPY=none` in `.env` starts the upgrade without a copy; then the backup of step 2 is the only way back.
- `a database with the name junto_before_<number> exists and does not have the comment of a copy of this database`: the server did not make that database, or a version of the server before this release was stopped while it made its copy. The server does not replace it. Give it another name or delete it (`docker compose -f one-host.yaml exec postgres dropdb -U junto junto_before_<number>`), and start again.
- `a database with the name junto_making_<number> exists, and this server did not record that it made it`: the same for the name under which the server creates its copy. A start that was stopped while it made the copy leaves such a database, and the next start deletes it when the comment of the database `junto` names it. After a way back that comment is on `junto_undone`, and the database is deleted by hand.
- `must be owner of`: an earlier release, in which the server connected as `junto`, created a table after the role of the server got the database. Run `deploy/database-user.sh` again.
- `a recording is starting or runs`: start the earlier release, end the recording, and repeat.
- `database migration failed`: go back as described below, and report the message. The migrations before the failed one stay applied. The comment of the database then states `junto: an upgrade of this database has not finished. junto_before_<number> is the copy from before the upgrade`, and a further start keeps that copy and makes no other: `docker compose -f one-host.yaml exec postgres psql -U junto -d postgres -Atc "select shobj_description(oid, 'pg_database') from pg_database where datname = 'junto'"`.

### The way back after an upgrade

To the copy, when the upgrade failed or the new release was found wrong before the server deleted files:

```sh
git checkout <earlier release>
deploy/upgrade-undo.sh one-host.yaml
(cd deploy && docker compose -f one-host.yaml up -d --build)
```

`upgrade-undo.sh` runs in the checkout of the earlier release. It stops the server and the worker, builds the server image of the checkout, gives the database role of that server the password of `.env`, and runs `junto-server new-event-epoch` on the copy, so that every client loads a new snapshot: the clients hold positions of events that the copy does not have (ADR 0010). Then it gives the copy the name `junto` and keeps the database that the new release changed as `junto_undone`, in one transaction. A failure before that transaction changes no name, and the script can be run again. Everything that was written after the upgrade is in `junto_undone` only. The script names the command that deletes `junto_undone`, and it changes nothing while a `junto_undone` of an earlier way back exists.

A checkout that is older than the script does not have it, and a checkout with an earlier form of the script does not read the comment of the database. Copy `deploy/upgrade-undo.sh` of the newer release into `deploy/` of that checkout before the script is run.

Without a name, the script takes the copy that the comment of the database names. The server writes that comment before the first migration and removes it after the last one, so it is there after an upgrade that failed. Without that comment the script takes the one database `junto_before_<number>` that has the comment of the server. A second argument names another one.

The copy is the database from before the first migration of the upgrade, also when the upgrade failed in a later migration and the server was started again: a start that finds an upgrade that has not finished keeps the copy and makes no other (ADR 0018).

The copy holds the database and no file. An attachment or a recording that was deleted after the upgrade is not brought back, and the earlier release then names a file that does not exist. In that case, and when the copy was deleted, restore the backup of step 2 with the checkout of the earlier release (see "Restore").

A directory that was written after the upgrade stays on the disk, and no row of the copy names it. When the server of the earlier checkout has the command, the script runs `junto-server unnamed-files` after the renames and prints these directories (see "Directories that the database does not name"). A release before that command prints its usage, and the directories stay.

## A changed secret

- `JUNTO_POSTGRES_PASSWORD` is stored by PostgreSQL at its first start, as the password of the role `junto_server`. When the line in `.env` no longer matches, the server does not start and its log states `password authentication failed for user "junto_server"`. To change the password, change it in the database first, then in `.env`, then run `up -d`. `psql` asks for the new value, so that it is not part of a command line: `docker compose -f <form file> exec postgres psql -U junto -d postgres -c '\password junto_server'`. Use letters and digits only.
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
- The address that the server sees for a request of the entry was not observed on a Linux host. `JUNTO_TRUSTED_PROXIES` names the address of the host in the container network. The setting also takes a network; the forms name one address, because the recorder and Redis are containers of the same network.
- The backup, the restore, the upgrade and `deploy/database-user.sh` were run in the local form only (`deploy/tests/backup-restore.sh`, `deploy/tests/upgrade.sh`), with recordings that the test wrote as rows and files, not with the recorder. In the one-host form and on the data host, the commands of the server binary run in the container of the server, which has a fixed address there.
- The way back to a release that has the command `unnamed-files` was run with a copy that the test made by hand, because no later release exists. The scripts of an earlier checkout were not run on a deployment that has the role `junto_server`, except the server of that checkout and `upgrade-undo.sh`.
- A recording through the recorder was made in the local form only (`deploy/tests/recording.sh`, ADR 0910), on one machine with Docker Desktop on arm64. In the one-host form and in the two-host form the recorder reaches the media server on its public name, and no recording was made there.
- The recorder ran without a capability in the local form. It was not run on a Linux host with another seccomp or AppArmor profile (ADR 0020).
- In the local form, `rtc.stun_servers` stopped the recorder's requests to public STUN servers of third parties. The one-host form names its TURN port in the same setting; it was not observed there (ADR 0007).
