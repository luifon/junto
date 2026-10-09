# Spike: media host — result

Run on 2026-10-09. Every statement is marked **observed** (run and seen) or
**not verified**. The media host is never named. Commands and scripts take its
SSH name from the environment variable `JUNTO_TEST_MEDIA_HOST`, and the SSH
name of the second load generator from `JUNTO_TEST_DATA_HOST`.

## Question

What can the first deployment's public host (1 CPU core, 4 GB RAM, Ubuntu
24.04) carry as a LiveKit server, and how does LiveKit behave there when UDP is
blocked, when Redis stops, and when a token expires or a participant is removed?

## Gates

| Gate | Result | Short statement |
|---|---|---|
| M1 Stack up | **PASS** | LiveKit 1.13.9 and Redis 7.2.16 run in containers; one UDP port, TCP fallback, keys generated on the host, Redis on loopback with a password. |
| M3 Call over UDP, and over TCP when UDP is blocked | **PASS** | UDP: observed (`connectionType: udp`). TCP on 7881: observed with a libwebrtc client. The `lk` tool cannot use the TCP fallback. |
| M5 Capacity | **PASS** (measured) | No server limit up to 80 subscribers of one 2.5 Mbit/s stream (CPU 22–28 %) and 16 video participants (CPU 23 %). Audio with 50 participants reaches 70–80 % CPU. Table below. |
| M6 Loss of Redis | **PASS** (facts observed; the behaviour is adverse) | Running calls stop about 6 s after Redis stops. New joins and the API fail. After Redis returns, one of two clients reconnected by itself; the other had given up. |
| M7 Token lifetime | **PASS** (facts observed; the behaviour is adverse) | A 60 s token is accepted until about 120 s. A removed participant can rejoin with the original token and with the 600 s token the server sent at connect. |
| M2 TLS plan | **Written, not run** | `tls-plan/`. No DNS change, no certificate request, nothing on 443 or 80. |

## Versions (observed)

| Component | Version |
|---|---|
| Media host | Ubuntu 24.04.4, kernel 6.8.0-110, 1 vCPU (x86-64), 3915 MiB RAM, no swap |
| Docker / Compose on the host | 29.4.1 / v5.1.3 |
| LiveKit server | `livekit/livekit-server:v1.13.9` |
| Redis | `redis:7.2.16-alpine` |
| LiveKit CLI (`lk`) | 2.18.8 (Homebrew on the Mac; `livekit/livekit-cli:v2.18.8` image in containers) |
| Python test participant | `livekit` 1.1.20, `livekit-api` 1.2.1 (libwebrtc through the LiveKit Rust FFI) |
| Mac | macOS 26.6.1, 8 cores, Wi-Fi, Docker Desktop 29.2.1, ffmpeg 9.0.1 |
| Second load generator | the data host, a second Linux machine reached over SSH: 16 cores, 32 GB, Docker 29.4.0 |

The Rust tool in `spikes/test-participant/` was not finished when the tests
ran and was not used.

## M1 — Stack up: PASS

Files: `host/compose.yaml`, `host/livekit.yaml`, `host/setup-host.sh`,
`host/env.example`. The copy on the host is in `/opt/junto-spike/`.

```
scp host/compose.yaml host/livekit.yaml host/setup-host.sh "$JUNTO_TEST_MEDIA_HOST":/opt/junto-spike/
ssh "$JUNTO_TEST_MEDIA_HOST" 'cd /opt/junto-spike && sh setup-host.sh && docker compose up -d'
```

`setup-host.sh` writes `/opt/junto-spike/.env` (mode 600) on the host with the
host's address, an API key, an API secret and a Redis password from
`openssl rand`. The three values `JUNTO_TEST_LK_API_KEY`,
`JUNTO_TEST_LK_API_SECRET` and `JUNTO_TEST_LK_URL` were appended to `.env.local`
on the Mac through a pipe; they were not printed.

Observed:

- LiveKit start line: `starting LiveKit server {"portHttp": 7880, "version":
  "1.13.9", "rtc.portTCP": 7881, "rtc.portUDP": {"Start":7882,"End":0}}` and
  `using explicit node IP for NAT1To1Ips`.
- Both containers: `restart=no`. LiveKit uses host networking; Redis is
  published as `127.0.0.1:6379->6379/tcp` only.
- `redis-cli ping` without a password: `NOAUTH Authentication required.` With
  the password: `PONG`.
- The two containers of an unrelated service that already ran on the host
  show `Up 5 months` before and after. They were not touched.
- LiveKit warns at start: `UDP receive buffer is too small for a production
  set-up {"current": 425984, "suggested": 5000000}`. `net.core.rmem_max` and
  `wmem_max` are 212992. The spike did not change this sysctl.

### Firewall rules added

```
ufw allow 7880/tcp comment junto-spike
ufw allow 7881/tcp comment junto-spike
ufw allow 7882/udp comment junto-spike
```

ufw created an IPv4 and an IPv6 rule for each (6 entries). No other rule was
added, changed or deleted. No package was installed on the host.

## M3 — A call over UDP and over TCP: PASS

**UDP (observed).** Two `lk room join --publish-demo --auto-subscribe`
participants from the Mac in one room. Both reported `track subscribed` for the
other's video; `lk room participants list` showed both `ACTIVE`. The server log
line `participant active` had `"connectionType": "udp"`, with the selected pair
`udp6 host …:7882` for the subscriber connection. The same test with two Python
participants: selected pair `remote_protocol: udp, remote_port: 7882`, 0
packets lost.

**TCP fallback (observed).** Clients ran in a Docker container on the Mac whose
network namespace drops all outbound UDP (`iptables -A OUTPUT -p udp -j DROP`).

- `lk` (LiveKit Go SDK): **does not connect.** `could not connect after
  timeout`; the server logged `removing participant without connection`.
- Python participant (libwebrtc): **connects.** Selected pair
  `remote_protocol: tcp, remote_type: HOST, remote_port: 7881`; server log
  `"connectionType": "tcp"` for both participants; 2 established TCP
  connections on 7881 on the host; 62 UDP packets counted as dropped in the
  client namespace; audio and video frames received in both directions, 0
  packets lost in 30 s.

Commands: `scripts/Dockerfile.participant`, `scripts/participant.py`; the
namespace container was `alpine:3.22` with `--cap-add NET_ADMIN`, and the
clients used `--network container:<that container>`.

## M5 — Capacity: measured

### Method

- `scripts/run-load.sh <label> <seconds> <lk load-test options>` runs the load
  and, during it, `host/sample.sh` on the host every 5 s: CPU of the LiveKit
  container from its cgroup, host CPU from `/proc/stat` (user, system, softirq,
  steal, idle), memory, interface throughput, UDP buffer errors. One
  `docker stats --no-stream` and one `top -b -n 1` per run confirm the values.
  The script needs `JUNTO_TEST_MEDIA_HOST`. With `SECONDGEN="<lk load-test
  options>"` it also runs a generator in a container on the second machine
  and needs `JUNTO_TEST_DATA_HOST`.
- "LiveKit CPU" is percent of the one core. "Host busy" is 100 − idle and
  includes softirq and steal. Averages leave out the join and leave phases.
- Loss and bitrate are the totals `lk load-test` prints.
- Series (a) publishes a 1080p, 15 frames/s H.264 file made with ffmpeg; the
  host received 2.5 Mbit/s from it. `lk`'s own "high" video is 1.2–1.5 Mbit/s,
  so it was not used for (a).
- Each run lasted 60 s (one 40 s). Total traffic on the host interface during
  the runs: 8.6 GB.

### Which machine generated the load, and why

The Mac was the first generator. With 20 subscribers (50 Mbit/s) `lk` on the
Mac reported 4.7 % loss while LiveKit used 9 % CPU. The same load from two
other places had no loss, so the loss is on the Mac's side (Wi-Fi path or the
Mac), not in the server:

| 20 subscribers × 2.5 Mbit/s, generator at | Loss reported | Bitrate received |
|---|---|---|
| Mac, one `lk` process | 4.75 % | 23.0 Mbit/s |
| Mac, four `lk` processes of 5 | 3.35 % | 27.1 Mbit/s |
| container on the media host | 0 % | 43.7 Mbit/s |
| container on the second machine | 0.06 % | 43.8 Mbit/s |

Not determined: whether the Mac loses the packets on the Wi-Fi path or in the
`lk` process. All later runs used the second machine. Its container counted
arriving packets with an iptables rule, which was compared with the packets
LiveKit counted as sent.

### (a) One 2.5 Mbit/s video publisher, N subscribers

| Subscribers | Generator | LiveKit CPU avg / max | Host busy avg / max | Sent by host | Received (`lk`) | Loss (`lk`) | Limit reached |
|---|---|---|---|---|---|---|---|
| 5 | Mac | 3 % / 5 % | 6 % / 11 % | 12.3 Mbit/s | 11.1 Mbit/s | 0.07 % | none |
| 10 | Mac | 6 % / 9 % | 10 % / 19 % | 25.4 Mbit/s | 20.6 Mbit/s | 0.72 % | load generator (Mac side) |
| 20 | Mac | 9 % / 14 % | 11 % / 23 % | 50.4 Mbit/s | 23.0 Mbit/s | 4.75 % | load generator (Mac side) |
| 20 | second machine | 7 % / 9 % | 9 % / 11 % | 48.3 Mbit/s | 43.8 Mbit/s | 0.06 % | none |
| 40 | second machine | 14 % / 30 % | 16 % / 38 % | 100.7 Mbit/s | 86.8 Mbit/s | 0.12 % | none |
| 80 (extra) | second machine | 22–28 % steady | 23–34 % steady | 210 Mbit/s | 167.3 Mbit/s | 0.33 % | none |
| 120 (extra, 40 s) | second machine | 36–42 % | 35–43 % | 320–370 Mbit/s | 165.3 Mbit/s | 1.7 % | network of the generator |

- 80 subscribers: one 5 s sample had 51 % steal (the hypervisor gave the core
  to another guest); host idle was 20 % in that sample. 1 382 766 packets
  arrived of 1 414 972 RTP packets LiveKit counted, so about 2 % were lost on
  the path before retransmission.
- 120 subscribers: 1 191 768 packets arrived of 2 091 417 counted as sent. The
  path to the generator carried about 280 Mbit/s. Server CPU was 36–42 %, so
  the server was not the limit.
- The series was not continued to a server limit; the network between the two
  test sites is reached first.

### (b) N participants publish 720p video with simulcast, all subscribe

`lk load-test --video-publishers N --subscribers N` from the second machine.
In this tool a publisher does not subscribe, so N participants are 2N
connections and each subscriber receives N tracks (a real call: N − 1). The
`--layout` option sets how many tracks a subscriber takes and at which
simulcast layer: `speaker` takes 6 tracks at most, `3x3` 9, `4x4` 16. LiveKit
asks publishers only for the layers that are subscribed, so the 720p layer was
not sent in the grid layouts (the host received 0.8–1.2 Mbit/s per publisher).

| Participants | Layout | LiveKit CPU avg / max | Host busy avg / max | Host received / sent | Tracks | Received (`lk`) | Loss (`lk`) | Limit reached |
|---|---|---|---|---|---|---|---|---|
| 4 | 3x3 | 6 % / 8 % | 9 % / 14 % | 4.0 / 10.9 Mbit/s | 16/16 | 9.6 Mbit/s | 0 % | none |
| 8 | 3x3 | 13 % / 20 % | 18 % / 25 % | 9.6 / 38.5 Mbit/s | 64/64 | 34.9 Mbit/s | 0.01 % | none |
| 12 | 4x4 | 15 % / 29 % | 20 % / 35 % | 10.0 / 33.3 Mbit/s | 144/144 | 27.8 Mbit/s | 0.04 % | none |
| 16 | 4x4 | 23 % / 37 % | 26 % / 40 % | 15.9 / 96.5 Mbit/s | 256/256 | 79.1 Mbit/s | 2.36 % | not server CPU; place of the loss not determined |
| 4 | speaker | 6 % / 9 % | 8 % / 14 % | 6.4 / 11.2 Mbit/s | 16/16 | 10.3 Mbit/s | 0 % | none |
| 8 | speaker | 12 % / 17 % | 14 % / 20 % | 15.3 / 24.3 Mbit/s | 48/64 (tool takes 6 of 8) | 21.7 Mbit/s | 0.01 % | none |

- 16 participants: host busy was 26 %, the host had 93 UDP send-buffer errors,
  and 685 114 packets arrived for 682 401 RTP packets counted as sent (RTCP
  packets are in the first number only). The server log for this run has 6
  `Incoming unhandled RTP ssrc … failed Simulcast probing` errors for `lk`
  publishers. A repeat with the generator on the media host itself used the
  whole core (host busy 82 % average, 100 % maximum, 251 877 UDP buffer
  errors) and is not a valid measurement of the server.
- Not measured: every subscriber receiving the 720p layer of every publisher.
  `lk load-test` has no such layout.

### (c) Audio only

`--audio-publishers N --subscribers N --layout 5x5`, every publisher sends
audio all the time (about 20 kbit/s, 50 packets/s per stream). The tool gives
a subscriber 25 tracks at most.

| Participants | Audio streams forwarded | LiveKit CPU avg / max | Host busy avg / max | Sent by host | Loss (`lk`) | Limit reached |
|---|---|---|---|---|---|---|
| 20 | 400 (every subscriber hears all 20) | 35 % / 50 % | 38 % / 54 % | 19.0 Mbit/s | 0 % | none |
| 50 | 1 250 (25 per subscriber; all 50 would be 2 500) | 70–80 % | 72–83 % | 61 Mbit/s steady | 0.27 % | server CPU |

- 50 participants: while the 100 connections joined (about 20 s) host idle
  fell to 17–19 % with up to 30 % steal, the host sent 21–33 Mbit/s instead of
  61, and it counted 9 131 UDP send-buffer errors. Over the run, 3 507 191
  packets reached the generator of 4 290 873 the server counted as sent; the
  bytes that arrived equal the bytes the host put on the wire, so the missing
  packets did not leave the host. After the join phase the CPU stayed at
  70–80 % for 40 s. The series stops here.
- Not measured: 50 participants who each receive all 50 streams. It is twice
  the forwarded streams of the measured run, which already used 70–80 %.
- Audio costs more CPU per Mbit than video: 1 250 small-packet streams at
  61 Mbit/s used about 75 % CPU; 80 video subscribers at 210 Mbit/s used about
  25 %. The cost follows the packet count and the number of connections.

### Memory (observed)

| | Value |
|---|---|
| LiveKit container idle after start | 27 MiB |
| LiveKit container, highest in the remote runs | 461 MiB (audio, 50 participants); 327 MiB (120 video subscribers) |
| LiveKit container after all tests, idle | 46 MiB |
| Redis container | 3–4 MiB |
| Host memory available, lowest in the remote runs | 2 923 MiB of 3 915 MiB |
| Host memory available at the end | 3 322 MiB |

LiveKit's memory did not fall back at once between runs, so a figure for one
run includes memory from the run before it.

### Other observations

- Steal time was 1–10 % on average in most runs and 51 % in one 5 s sample.
- LiveKit logged `high cpu load` (0.97–0.99) for about one second when all
  test participants left at the same moment.

## M6 — Redis stops during a call (observed)

`scripts/m6.sh`: two Python participants publish audio and video to each
other; `docker stop junto-spike-redis` at 22 s; `docker start` at 51 s. The
script needs `JUNTO_TEST_MEDIA_HOST`.

| Question | Observed |
|---|---|
| Do existing calls continue? | **No.** 6 s after Redis stopped both clients changed to `RECONNECTING`, and received no more audio or video. Server log: `could not send signal message … "request timed out"`, then `could not get node for room: dial tcp …:6379: connect: connection refused` for every reconnect attempt. |
| Can a new participant join? | **No.** `signal failure: handshake error: 500 Internal Server Error`. |
| Does the server API work? | **No.** `lk room list` and `participants list`: `twirp error unknown: … connection refused`. |
| Recovery after `docker start` | The API answered 3 s later, with an empty room list (Redis keeps nothing on disk in this setup). One client reconnected by itself 3 s after Redis returned and got a new token. The other client ended as `DISCONNECTED` 28 s after the break, 6 s after Redis was back, and did not reconnect; it has to join again. A new participant joined the room normally. |

The media outage lasted from 6 s after the stop until the client reconnected:
about 25 s for a 29 s Redis outage.

## M7 — Token lifetime facts (observed)

`scripts/m7.sh` and one further run for the expiry limit. The clocks of the Mac
and the host differed by 0 s.

| Fact | Observed |
|---|---|
| Join with a 60 s token after it expired | Accepted 10 s, 43 s and 54 s after expiry. Rejected 65 s, 75 s and 85 s after expiry with `401 Unauthorized - invalid authorization token: … token is expired`. A 60 s token works for about 120 s. |
| Token refresh | The server sends a new token to the client 0.01 s after it connects. Its lifetime is 600 s, whatever the lifetime of the token used to join. Same identity, same room. |
| A participant connected with a 60 s token | Stays connected after the token expires (75 s observed). |
| `RemoveParticipant` | The client is disconnected with reason `PARTICIPANT_REMOVED`. |
| Rejoin with the original token inside its lifetime | **Succeeds** (46 s of lifetime left). |
| Rejoin with the original token 16 s after expiry | **Succeeds** (inside the 60 s tolerance). |
| Rejoin with the refreshed token | **Succeeds** 80 s after the removal, with 520 s left. The new connection again receives a 600 s token. |

What a controller must do to keep a removed person out:

1. `RemoveParticipant` does not invalidate any token. After a removal the
   controller must list the room's participants repeatedly and remove the
   person again each time, for as long as that person holds a valid token.
2. That time is not the join token's lifetime. It is 600 s plus about 60 s
   after the last connection of that person, and each successful rejoin starts
   it again. In practice the check has to run for as long as the room exists.
3. A join token's lifetime of N seconds must be treated as N + 60 seconds.
4. Not verified: ending the room and continuing in a room with a new name
   (tokens carry the room name), with `room.auto_create: false` so that an old
   token cannot create the old room again. Not verified: any LiveKit setting
   that shortens the refreshed token's 600 s.

## M2 — TLS plan (design only)

`tls-plan/caddy.yaml`, `tls-plan/livekit.additions.yaml`,
`tls-plan/compose.addition.yaml`, `tls-plan/NOTES.md`. Caddy with the layer4
module selects by server name on 443; it obtains and renews the three
certificates; TURN does not need TLS passthrough when `turn.external_tls` is
true. Sources are listed in `NOTES.md`. Nothing was run.

## What the product's deployment needs

1. **Redis is part of the call path.** A Redis stop ends every call within
   seconds, on one node too. Redis needs a restart policy, a health check, and
   a place in the chat server's health endpoint. The client must rejoin by
   itself when its SDK ends the reconnection attempts (after 28 s in this test).
2. **The repeated admission check is required, and for the room's whole
   life.** See M7. Join-token lifetime does not limit a removed person.
3. **UDP buffers.** Raise `net.core.rmem_max` and `net.core.wmem_max` to the
   5 000 000 that LiveKit asks for. With the default, the host counted UDP
   send-buffer errors under load.
4. **The TCP fallback works with libwebrtc clients only in this test.** The Go
   SDK did not use it. Tests of the fallback need the Rust client. TURN over
   TLS on 443 (M2) is still needed for networks that allow only 443.
5. **Firewall:** 7881/tcp and 7882/udp, IPv4 and IPv6 (clients used IPv6 for
   media when they had it). 7880/tcp is open only until the entry on 443
   exists. LiveKit's metrics port (6789) listens on all addresses with host
   networking; the firewall must keep it closed.
6. **Capacity.** A call of the team's size uses a few percent of the core: 5
   viewers of a 2.5 Mbit/s screen share 3 %, 8 video participants 13 %. The
   costly case is many people sending audio at the same time (20: 35 %, 50
   with 25 streams each: 70–80 %), and many participants joining in the same
   seconds. Steal time on this host reached 51 % in one sample.
7. **Not measured here, and still open:** the same load with a recording
   running, with TURN relaying, with the HTTPS entry and the private link on
   the same core, and with all subscribers on the 720p layer.
8. **Restart policy.** The spike uses `restart: "no"`. The product needs
   `unless-stopped` for LiveKit and Redis.
9. **Redis reachable from the data host.** The recorder connects to Redis over
   the private link, so Redis must also listen on the link address. The spike
   has loopback only.
10. **Redis licence.** The spike uses 7.2.16, the last line under the BSD
    licence. Newer Redis versions have other licences. The product has to
    choose a version or a compatible server.

## What is left in place

The stack is **running** on the media host with `restart: "no"`:
`junto-spike-livekit` and `junto-spike-redis`, files in `/opt/junto-spike/`
(`compose.yaml`, `livekit.yaml`, `.env`, `setup-host.sh`, `sample.sh`), and the
three ufw rules. Left for the other tests that use this stack. A join was
confirmed after the last test.

On the Mac: three lines in `.env.local`; `out/` in this directory (raw logs,
2 MB, not tracked, addresses masked); `livekit-cli` from Homebrew.

Removed: all `junto-spike-*` containers of this spike on the Mac and on the
second machine, the client and CLI images on the Mac, the CLI image on the
host and on the second machine, the Python environment, the test video file,
and the saved test tokens. The image `alpine:3.22` stays on the Mac and on the
second machine (small, and possibly used by other tests).

To run the scripts again:

```
uv venv out/venv && uv pip install --python out/venv/bin/python livekit==1.1.20 livekit-api==1.2.1
../../tools/serial.sh ffmpeg -f lavfi -i "testsrc2=size=1920x1080:rate=15" -t 100 -c:v libx264 -preset veryfast \
  -profile:v baseline -pix_fmt yuv420p -bf 0 -g 30 -keyint_min 30 -sc_threshold 0 -b:v 2850k -minrate 2850k \
  -maxrate 2850k -bufsize 1425k -x264-params "nal-hrd=cbr:force-cfr=1" -bsf:v h264_mp4toannexb -f h264 out/media/screen-1080p15.h264
PROCS=0 SECONDGEN="--subscribers 40" PUBLISH_FILE=out/media/screen-1080p15.h264 scripts/run-load.sh a40 60
scripts/summarize.sh a40
```

## Commands to stop and remove everything

```
ssh "$JUNTO_TEST_MEDIA_HOST" 'cd /opt/junto-spike && docker compose down -v'
ssh "$JUNTO_TEST_MEDIA_HOST" 'ufw delete allow 7880/tcp && ufw delete allow 7881/tcp && ufw delete allow 7882/udp'
ssh "$JUNTO_TEST_MEDIA_HOST" 'ufw status numbered'        # check: no line with the comment "junto-spike" remains
ssh "$JUNTO_TEST_MEDIA_HOST" 'rm -rf /opt/junto-spike'
ssh "$JUNTO_TEST_MEDIA_HOST" 'docker rmi livekit/livekit-server:v1.13.9 redis:7.2.16-alpine'
```

Then delete the lines `JUNTO_TEST_LK_API_KEY`, `JUNTO_TEST_LK_API_SECRET` and
`JUNTO_TEST_LK_URL` from `.env.local`. Not verified: that `ufw delete allow
<port>` matches a rule that has a comment; if it reports "Could not delete
non-existent rule", delete by the numbers that `ufw status numbered` shows for
the six `junto-spike` lines, highest number first.
