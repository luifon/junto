# 0020. The recorder container: rights, reach, credentials, and the room of a call

## Status

Accepted

Open: the recorder ran with these rights in the local form on one machine (ADR 0910). The one-host form and the two-host form were not run with a recording. The browser of the recorder runs without its sandbox.

## Context

The recorder is LiveKit Egress. It joins a room, decodes the media that people send, and loads the layout page in a browser (ADR 0602). ADR 0015 kept the rights that step 0 ran it with, the default capabilities of Docker and `SYS_ADMIN`, and stated that a reduction needs a recording as its test. ADR 0910 adds that test. A first recording through the deployment showed these facts:

- The recorder image runs as the user 1001 with the group 0. It contains programs that run as root when a user starts them (`su`, `mount`, the sandbox helper of the browser).
- The recorder starts its browser without the sandbox of the browser. The setting `enable_chrome_sandbox` of the recorder turns the sandbox on; the recorder documents that it then needs a changed seccomp profile of the container.
- The log of the recorder at the level `info` holds no room token.
- In the local form the log of the recorder named two public STUN servers of a third party. The media server gives its clients these servers when its configuration names none (ADR 0007).
- The media server closes a room 20 seconds after its last person left. The server ends a call 20 seconds after the admission check saw its last person absent (ADR 0509). When the last person of a recorded call lost the connection, both happened in the same second: the room closed, the composite lost its participant before the server had sent its stop, and the server declared the job lost (ADR 0604).

The recorder needs the key and the secret of the media server, because it creates the token with which each job joins its room. LiveKit has no credential with fewer rights for a recorder.

## Decision

Rights:

- The recorder runs as the user 1001, the user of its image, which the Compose file names, with the groups that the image gives that user. Every capability is dropped, and the flag that forbids new privileges is set. `SYS_ADMIN` is removed. A program of the image that is marked to run as root runs as the user 1001.
- Its root file system stays writable. During a recording the recorder wrote below the home directory of its user (`.cache`, `.config`, `.local`) and below `/tmp`. A read-only root with memory file systems at those places was not tried.
- The sandbox of the browser stays off, as the image starts it. Turning it on needs a seccomp profile that allows the browser to create namespaces; this was not run.

Reach:

- The recorder is on the network `backend` and not on the network of the database. It reaches the HTTP port of the server, Redis and the media server. It does not reach PostgreSQL.
- `backend` is not an internal network, because the recorder reaches the media server on its public address in the one-host form and in the two-host form (ADR 0003). The recorder therefore reaches the Internet and every network that its host routes to. It does not reach a port that listens on the loopback address of the host only.
- On the HTTP port of the server the recorder reaches every route: the public API, as every client on the Internet reaches it through the entry, and the two paths that the entry does not forward. `/recording-layout/` asks for no credential and serves public files (ADR 0615). `/internal/media/notifications` accepts a notification that is signed with the key of the media server (ADR 0508); the recorder holds that key. A notification makes the server read a room again and gives no access to data.
- The server trusts a forwarded client address only from the addresses of `JUNTO_TRUSTED_PROXIES`, which do not include the recorder.
- The recorder mounts the volume of the recordings and the volume of its temporary directory with write access, and its start script as one file that it cannot write. It mounts nothing else.

Credentials:

- The recorder holds the key and the secret of the media server and the password of Redis, in the variable `EGRESS_CONFIG_BODY` of its container. It holds no credential of the server and none of the database.
- The room token of a job is part of the address of the layout page in the browser of the recorder. The log level of the recorder stays above `debug` (ADR 0003), and the server does not log the query of a request (ADR 0615).

STUN:

- Every form names a STUN server of the deployment in `rtc.stun_servers` of the media server. The one-host form and the public host name the TURN port (ADR 0015). The local form names the media port on the loopback address, which answers no STUN request; on one machine no client needs one.

The room of a call:

- The media server keeps a room for 60 seconds after its last person left (`room.departure_timeout`), in every form. The server ends the call 20 seconds after its admission check saw that person absent, and then ends the jobs of the recorder, while the room and the participants of the recorder still exist.

## Consequences

- A person who takes over the recorder, through the page that it loads or the media that it decodes, has no capability in the container and cannot become root through a program of the image. This person reads the key and the secret of the media server from the environment of the recorder process, because the browser runs as the same user and without its sandbox.
- With that key and secret a person creates a token for every room, joins every call, removes participants and starts jobs of the recorder. With the password of Redis the person reads, changes and deletes what the media server keeps there about its rooms, which ends calls (ADR 0003). The recorder is trusted as far as the media server is. Neither secret opens the API of the server or the database.
- A person who takes over the recorder reads, changes and deletes every file of the volume of the recordings. The server follows no link there and checks every file that it reads (ADR 0606); the worker reads the audio files with its own limits (ADR 0610).
- With the recorder's capabilities removed, a recording, a recording that everybody left and a recording with a killed recorder ended as with them (ADR 0910).
- With `rtc.stun_servers` set, the log of the recorder named that server and no other. The log line of the failed request to the loopback address appears once for each job.
- A room without people stays for one minute. The server lets nobody into a room without a call (ADR 0509).
- A recording whose last person lost the connection ends with complete files and without a failed segment. In the run before the change of the timeout its composite was declared lost, and the recording was `partly_failed`.
- The 40 seconds between the two limits hold while the admission check runs at its interval. A server that is stopped, or that does not reach the media server, for longer than that ends the call after the room has closed; the rule of ADR 0604 for a job without a participant then decides.
- The reduction to no capability was run on Docker Desktop with the image `livekit/egress:v1.15.0`. On a Linux host with another seccomp or AppArmor profile it was not run.
