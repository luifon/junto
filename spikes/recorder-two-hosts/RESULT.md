# Recorder on two hosts: result

Date of the test: 2026-10-09.

## Question

In the two-host form (ADR 0003) the LiveKit server and Redis run on the public
host, and LiveKit Egress runs on the data host, which can open connections but
cannot receive them. A WireGuard link opened from the data host joins the two.

1. Does a recording work in this arrangement when Redis is reachable for the
   data host only through the link?
2. Which recording configuration loses the least when the Egress process is
   killed, and how does a controller detect the loss?
3. What do a link outage, the timeline of the files, the cost and a silent
   failure look like in this arrangement?

## Result per gate

| Gate | Result | Short statement |
|---|---|---|
| X1 private link in containers | PASS | Userspace WireGuard in one container per host. Redis answers the data host through the link (13 ms average). From the Mac the Redis port times out on IPv4 and IPv6. Two findings: Docker's published UDP port worked before the `ufw` rule existed, and the first version of the link container made the kernel load its `wireguard` module on both hosts (see "Deviation"). |
| X2 recording across hosts | PASS | 1080p30 composite and one Ogg file per audio track, recorded on the data host from a call on the public host. Main picture and tones are correct in 7 of 7 calls. |
| X3 surviving a recorder crash | measured | MP4 output: nothing playable survives. Segment output: every finished segment is in the final location; the composite loses 4.2 to 4.5 s (2 s segments, 1080p). Track egress Ogg files survive in Egress's working directory and play; they lose 1.0 to 3.0 s when that directory is saved before Egress starts again. |
| X4 detecting a lost recorder | PASS | `ListEgress` reports `EGRESS_ACTIVE` without end. The recorder's participant leaves `ListParticipants` 19.4 to 21.6 s after the kill. `StopEgress` answers HTTP 408 after 3.0 s. A new recording in the same room worked 2.3 s after the crash. |
| X5 link loss | PASS | 30 s without the link: the call continued without a missing second, all running recordings continued and their files are complete. `StopEgress` failed during the outage (408). A state change of Egress from the outage reached LiveKit 16 s after the link returned. |
| X6 timeline | PASS with a method | Track files: 41 to 56 ms late, in every one of 24 files. Composite: its content begins 1.49 to 1.96 s before the start time that Egress reports. Placing the composite by its end time and its media length gives an error of -15 to +26 ms in 9 recordings. |
| X7 cost | measured | Egress, 1080p30 composite plus five track files, quiet data host: 2.5 cores on average, 2.9 at peak (segments), 1.36 GiB. The public host's LiveKit CPU did not rise with a recording: 11.9 % without, 9.4 % and 10.3 % with. Load points with a recording: 25.7 to 28.0 % (40 subscribers; 23.0 and 28.0 % without) and 31.9 % (16 publishers; 28.8 % without). |
| X8 silent failure check | PASS | A check of packet timestamps, digital silence and key frames found all 6 defective files and passed all good files. It took 3.3 s for a 130 s file and 80 s for a 61 minute file. The earlier silent failure was reproduced: Egress limited to 0.5 CPU for 25 s reported `EGRESS_COMPLETE` with no error, and the file has 21.7 s of holes in its sound. |

Every statement below is marked **observed** (it was run and seen in this
test) or **not verified**.

## Deviation from the limits of the test: a kernel module was loaded

The limits of this test forbid loading a kernel module on the data host. The first version
of the link container did it on both hosts, without intent. **Observed:**

- The container's start script called `wg show wg0` in a loop until the
  userspace WireGuard process had created its control socket. While the socket
  does not exist, `wg` asks the kernel for the interface over generic netlink,
  and the kernel then loads its own `wireguard` module.
- Data host: `wireguard`, `libcurve25519`, `ip6_udp_tunnel` and `udp_tunnel`
  were loaded at 05:11:04 UTC, the second in which the link container
  first started. Public host: `wireguard` and eight modules it needs were
  loaded at 05:11:00 UTC, the second of its first start. The times are those of
  the directories under `/sys/module` and of the kernel log line `wireguard:
  WireGuard 1.0.0 loaded`.
- The link did not use the kernel module: when the loading was noticed, the
  module's use count was 0 and no interface of type `wireguard` existed on
  either host. The containers ran `wireguard-go`, which creates a TUN device
  (seen as `tun type tun` in the later verification run).
- The loading was noticed at the end of the test, during the removal.
- Correction: the start script now waits for the socket file
  (`/var/run/wireguard/wg0.sock`) and calls `wg` only afterwards
  (`link/entrypoint.sh`).
- Repair: on each host the modules that had been loaded in that second and had
  a use count of 0 were unloaded with `rmmod` (`wireguard` first). This was a
  command on the host, outside the containers. It was run because the limits
  of this test require that nothing of the test remains. After it, `lsmod` shows none of them
  on either host.
- Verification of the correction: with the modules unloaded, the link was
  started again on both hosts with the corrected script. The handshake
  completed, Redis answered `PONG` through the link, a 45 s call was recorded
  (composite and four track files, all `EGRESS_COMPLETE`, all pass the check
  of X8), and `lsmod` showed no `wireguard` module on either host afterwards.

All gate results except this last verification were measured with the first
version of the script, that is, with the unused kernel module loaded.

## Versions and environment

| Component | Version |
|---|---|
| Public host | Ubuntu 24.04, kernel 6.8.0-110, 1 vCPU, 3915 MiB, Docker 29.4.1, Compose v5.1.3 |
| Data host | Linux kernel 7.2.0, x86-64, 8 cores / 16 threads, 32 GB, Docker 29.4.0, Compose 5.4.0, behind carrier-grade NAT |
| LiveKit server (already running on the public host) | `livekit/livekit-server:v1.13.9` |
| Redis (already running on the public host) | `redis:7.2.16-alpine` |
| LiveKit Egress | `livekit/egress:v1.15.0` (linux/amd64); it contains Chrome 154.0.8037.57 and GStreamer 1.24.13 |
| Link container | `alpine:3.22` with `wireguard-go` 0.0.20250515 (MIT), `wireguard-tools` 1.0.20250521 (the `wg` program, GPL-2.0, run as a separate program), `socat` 1.8.1.3 (GPL-2.0, run as a separate program) |
| Layout page server | `nginx:alpine` (nginx 1.31.6) with `livekit-client` 2.22.3 |
| Tools container | `rust:1-bookworm` (rustc 1.99.0), clang 21.1.8, Python 3.11.2 with numpy 1.24.2, ffmpeg 5.1.9, Docker CLI 29.9.0 |
| Test participant | `../test-participant`, `livekit` 0.9.4, built on the data host |
| Load generator | `livekit/livekit-cli:v2.18.8` |

- The kernel configuration of both hosts has `CONFIG_WIREGUARD=m`. The module
  was not loaded before the test (**observed**, `lsmod`). Using it would have
  been a host change, so the link uses the userspace implementation.
- **Building the test participant on Linux (observed):** `webrtc-sys` 0.3.48
  stops with `clang++ is clang 14, but the hermetic libc++ shipped with this
  libwebrtc requires clang 21 or later`. `tools/Dockerfile` installs clang 21
  from apt.llvm.org and sets `CC` and `CXX`. The build then took 1 min 45 s.
- **Other work on the data host (observed):** other tests used the data host
  during most of this test. Until about 06:20 UTC its 1-minute load
  average was 8 to 32, and the kernel's CPU pressure figure
  (`/proc/pressure/cpu`, `some avg10`) was 11 to 36 % on average during the
  runs. From about 03:30 to 03:55 it was quiet (load 1 to 9, pressure 2 to 5 %
  while one recording ran). After that other work started again. Each result
  below states which condition it was measured in.
- **Other rooms on the LiveKit server (observed):** `ListRooms` showed only
  rooms of this test at every check.

## The arrangement that was tested

```
public host                                   data host (behind NAT)
-----------                                   ---------------------
junto-spike-livekit  (host network)           junto-spike-rec-egress   Egress + Chrome
  7880/tcp signalling, 7882/udp media  <----    ws://<public address>:7880, media over UDP 7882
junto-spike-redis                             junto-spike-rec-layout   layout page (http://layout/)
  published on 127.0.0.1:6379 only            junto-spike-rec-ctl      test participants, controller scripts
junto-spike-link                              junto-spike-rec-link
  wireguard-go, UDP 51820 published    <----    wireguard-go, opens the link, keepalive 25 s
  socat: <link address>:6379 -> redis:6379      socat: 0.0.0.0:6379 (Compose network only) -> <public end of the link>:6379
```

- Egress has `redis.address: link:6379`. That name exists only on the Compose
  network of the data host. The link carries nothing except Redis traffic.
- Egress reaches LiveKit as a client does: WebSocket to port 7880 and media
  over UDP to port 7882 of the public address. **Observed** in the Egress log:
  the selected pair is `udp4 host ... <-> udp4 host <public address>:7882`.
- The layout page is served by `junto-spike-rec-layout` on the data host.
- The link container of the public host joins the existing stack's Docker
  network to reach the name `redis`. No file of the existing stack was changed.
- In this test the controller scripts called the LiveKit API on the public
  address, port 7880. **Not verified:** API calls and signalling through the
  link.

## Files of this spike

| Path | Content |
|---|---|
| `link/` | Image of one end of the link: `wireguard-go`, `wg`, one `socat` forwarder. Used on both hosts. |
| `public-host/compose.yaml`, `env.example` | The link container of the public host. |
| `public-host/sample.sh` | The sampler of `../media-host/host/sample.sh` with Unix time in the first column. |
| `data-host/compose.yaml`, `env.example` | Link, layout page, Egress and the tools container on the data host. |
| `data-host/egress-entrypoint.sh` | Copies a killed Egress's partial files to `/out/rescued/` before the image's own entrypoint deletes them. |
| `layout/` | The layout page of `../recorder/layout` with three added flags: `lowtiles`, `novideo`, `nosound`. |
| `tools/Dockerfile` | Image of the tools container. |
| `requests/` | Request bodies: composite as segments, composite as MP4 and segments, track egress, audio track as segments. |
| `scripts/harness.py` | Shared code of the test calls: participants, egress requests, a log of every change in `ListEgress` and `ListParticipants` once per second. |
| `scripts/call_x2.py`, `analyze.py`, `align.py`, `timeline_summary.py` | The recorded call (X2, X6) and its analysis. |
| `scripts/call_kill.py`, `call_rescue.py`, `survey_kill.py`, `recover_segments.sh` | Crash tests (X3, X4) and the recovery of segments. |
| `scripts/call_link.py`, `analyze_link.py` | Link outage (X5). |
| `scripts/x7_cost.py`, `x7_load.py`, `x7-run.sh`, `x7_summary.py` | Cost and load points (X7). |
| `scripts/call_x8.py`, `check_recording.py` | Defective recordings and the result check (X8). |
| `scripts/x1-check.sh`, `rooms.py`, `show_log.py`, `call_short.py`, `lkapi.py`, `tones.py`, `fix_x2_phases.py` | Smaller tools. `lkapi.py` and `tones.py` are copies from `../recorder/scripts`. |
| `out/` | Not tracked. Controller logs, analysis results, extracted frames, selected log lines, the outputs of the removal. No media files. Addresses are replaced. 14 MB. |

## Commands

The commands and the scripts `scripts/x1-check.sh` and `scripts/x7-run.sh`
take the SSH name of the public host from the environment variable
`JUNTO_TEST_MEDIA_HOST` and the SSH name of the data host from
`JUNTO_TEST_DATA_HOST`.

Public host (`/opt/junto-spike-link/`, removed at the end):

```sh
rsync -a link public-host/compose.yaml "$JUNTO_TEST_MEDIA_HOST":/opt/junto-spike-link/
ssh "$JUNTO_TEST_MEDIA_HOST" 'cd /opt/junto-spike-link && docker build -t junto-spike-link ./link'
ssh "$JUNTO_TEST_MEDIA_HOST" 'docker run --rm -v /opt/junto-spike-link/keys:/keys junto-spike-link genkey'   # prints the public key
# write /opt/junto-spike-link/.env from public-host/env.example, then:
ssh "$JUNTO_TEST_MEDIA_HOST" 'cd /opt/junto-spike-link && docker compose up -d'
ssh "$JUNTO_TEST_MEDIA_HOST" 'ufw allow 51820/udp comment junto-spike-link'
```

Data host (`/srv/junto-spike-rec/`, removed at the end):

```sh
rsync -a link layout tools scripts requests "$JUNTO_TEST_DATA_HOST":/srv/junto-spike-rec/
rsync -a --exclude target ../test-participant/ "$JUNTO_TEST_DATA_HOST":/srv/junto-spike-rec/src/test-participant/
rsync -a data-host/compose.yaml data-host/egress-entrypoint.sh "$JUNTO_TEST_DATA_HOST":/srv/junto-spike-rec/
ssh "$JUNTO_TEST_DATA_HOST" 'cd /srv/junto-spike-rec && docker build -t junto-spike-rec-link ./link'
ssh "$JUNTO_TEST_DATA_HOST" 'docker run --rm -v /srv/junto-spike-rec/keys:/keys junto-spike-rec-link genkey'
# write /srv/junto-spike-rec/.env from data-host/env.example, then:
ssh "$JUNTO_TEST_DATA_HOST" 'cd /srv/junto-spike-rec && docker compose up -d'
ssh "$JUNTO_TEST_DATA_HOST" 'docker exec junto-spike-rec-ctl sh -c "cd /work/src/test-participant && cargo build --release"'
```

The private keys were created by `wg genkey` inside a container on each
machine and stayed there (`keys/private`, mode 600). Each public key and the
Redis password went from one machine to the other through a pipe on the Mac
(`ssh A '...' | ssh B 'cat >> .env'`). The API key and secret went from
`.env.local` on the Mac to the data host in the same way. None of these values
was printed or written to another file on the Mac.

Test runs, each inside the tools container (`docker exec junto-spike-rec-ctl
sh -c "cd /work/scripts && python3 ..."`). Each writes to `out/<room>/`:

```sh
./scripts/x1-check.sh                                  # on the Mac
python3 call_x2.py rth-x6d H264_1080P_30 2 ; python3 analyze.py rth-x6d ; python3 align.py rth-x6d
python3 call_kill.py rth-kill-plain plain   ; python3 survey_kill.py rth-kill-plain
python3 call_kill.py rth-kill-var variants  ; python3 survey_kill.py rth-kill-var
python3 call_kill.py rth-kill-product product ; python3 survey_kill.py rth-kill-product
python3 call_rescue.py rth-rescue           ; python3 survey_kill.py rth-rescue
python3 call_link.py rth-link 30            ; python3 analyze_link.py rth-link
python3 call_x8.py rth-x8
./scripts/x7-run.sh rth-x7q 290 x7_cost.py rth-x7q     # on the Mac; samples the public host at the same time
./scripts/x7-run.sh rth-load-a40r 110 x7_load.py a40r a40 rec
./scripts/x7-run.sh rth-load-b16l 110 x7_load.py b16l b16 rec lowtiles
python3 timeline_summary.py rth-x2 rth-x6a rth-x6b rth-x6c rth-x6d rth-x6e rth-x6f rth-x7q   # on the Mac
```

Request bodies that the controller sent (`ROOM` is the room name):

```json
StartRoomCompositeEgress, segments only (the configuration recommended in X3)
{"room_name": "ROOM", "layout": "", "custom_base_url": "http://layout/", "preset": "H264_1080P_30",
 "segment_outputs": [{"filename_prefix": "/out/ROOM/hls/composite", "playlist_name": "composite.m3u8",
                      "live_playlist_name": "composite-live.m3u8", "segment_duration": 2}]}

StartRoomCompositeEgress, MP4 and segments from one egress (X2, X6)
the same with "file_outputs": [{"file_type": "MP4", "filepath": "/out/ROOM/composite-{time}.mp4"}] added

StartTrackEgress
{"room_name": "ROOM", "track_id": "TR_...", "file": {"filepath": "/out/ROOM/tracks/alice-microphone-TR_...-{time}"}}

StartTrackCompositeEgress, one audio track as segments (X3)
{"room_name": "ROOM", "audio_track_id": "TR_...",
 "segment_outputs": [{"filename_prefix": "/out/ROOM/hls/alice-mic-seg2", "playlist_name": "alice-mic-seg2.m3u8",
                      "live_playlist_name": "alice-mic-seg2-live.m3u8", "segment_duration": 2}]}
```

Participants, tones and the 10 s time marker are those of `../recorder`
(alice 440 Hz and 1000 Hz, bob 620 Hz and 1370 Hz, carol 790 Hz and 1730 Hz,
all at -12 dBFS). All participants ran on the data host, so their clock is the
clock of Egress.

## X1. Private link in containers: PASS

Output of `scripts/x1-check.sh` is in `out/x1-check.txt`. **Observed:**

| Check | Result |
|---|---|
| Link state on the data host | `latest handshake` 4 s after the first start; 0.61 s after a later `docker start` of the container |
| Ping through the link, data host to public host | 11.4 to 18.9 ms, 0 of 6 lost |
| Redis through the link, with the password | `PONG`; `redis-cli --latency`: 11 ms minimum, 13.2 ms average, 17 ms maximum |
| Redis through the link, without the password | `NOAUTH Authentication required.` |
| Data host to port 6379 of the public address, without the link | `Operation timed out` |
| Mac to port 6379 of the public address, IPv4 | `Operation timed out` |
| Mac to port 6379 of the public address, IPv6 | `Operation timed out` |
| Mac to ports 7880 and 22 (IPv4) and 22 (IPv6) of the same address | `succeeded` (the path to the host works) |
| Listening sockets for 6379 on the public host | `127.0.0.1:6379` in the host's namespace, and `10.99.0.1:6379` (the link address) inside the link container. No other. |
| Published ports of the link container | `51820/udp` only |

- The link container of the public host has `NET_ADMIN` and `/dev/net/tun`,
  0.5 CPU and 128 MB. In 89 minutes it used 7.9 s of CPU time (0.15 % of the
  core) and 6 MiB. The link carried 0.7 MB in one direction and 0.5 MB in the
  other in that time (**observed**, `wg show wg0 transfer`).
- The link has one allowed peer address on each side (`/32`). The forwarder of
  the public host listens on the link address only. IP forwarding is not
  enabled in either container.
- **The `ufw` rule had no effect on reachability (observed).** The link
  completed its handshake and carried Redis traffic before any rule existed.
  Docker's rules for a published port are evaluated before ufw's. The rule
  `ufw allow 51820/udp comment junto-spike-link` was added afterwards, as the
  test plan asks (two entries, IPv4 and IPv6), and removed at the end. The last
  verification run (see "Deviation") ran without the rule and worked.
- **Not verified:** the kernel implementation of WireGuard, the link's
  throughput, and a link container in the host's network namespace.

## X2. Recording across hosts: PASS

Seven calls with `call_x2.py` (`rth-x2`, `rth-x6a` to `rth-x6f`). Each: alice
and bob join and share (bob's share starts 4 s later), a 1080p30 composite
with the custom layout page starts at 12 s (MP4 file and segments from one
egress), one track egress per audio track, carol joins at 60 s and shares at
65 s, carol leaves at 98 s, `UpdateLayout` with `main:alice` at 110 s, stop at
145 s. The figures below are from `rth-x6d` (quiet data host).

Composite sound (**observed**), median level in dBFS per period; a present
tone is near -12, an absent tone is below -60:

| Period in the file | Expected main share | 440 alice mic | 620 bob mic | 790 carol mic | 1000 alice share | 1370 bob share | 1730 carol share |
|---|---|---|---|---|---|---|---|
| 4 to 47 s: two shares, no instruction | bob (started last) | -12.0 | -12.0 | -83.1 | -87.4 | **-12.0** | -88.0 |
| 51 to 80 s: carol's share started | carol | -12.0 | -12.0 | -12.1 | -83.4 | -92.8 | **-12.0** |
| 84 to 92 s: carol left | bob | -12.0 | -12.0 | -81.6 | -86.8 | **-12.0** | -88.8 |
| 96 to 126 s: `UpdateLayout main:alice` | alice | -12.0 | -12.0 | -83.2 | **-12.0** | -88.4 | -95.0 |

The same four periods have the same result in `rth-x6a`, `b`, `c`, `e` and `f`,
for the MP4 and for the segments (`out/rth-x6*.analysis.txt`). In `rth-x2`
carol stayed until the end because of an error in the first version of the
script; its three periods are correct too.

Composite picture (**observed**, frames extracted and viewed, in
`out/rth-x6d/frames/` and `out/rth-x2/frames/`): period 1 shows `SCREEN BOB`
(red) as the main picture with the tiles of both cameras and alice's share;
period 2 shows `SCREEN CAROL` (green) with five tiles; period 4 shows `SCREEN
ALICE` (blue) and the page's status line `main=alice by=update-layout`.

Composite files (**observed**): MP4 with H.264 Main 1920x1080 at 30 frames/s
(2.5 Mbit/s for this content) and AAC-LC 44.1 kHz stereo (112 kbit/s),
131.9 s, 43.4 MB. The segments: 66 MPEG-TS files of 2 s with the same streams,
46.1 MB, a playlist with `#EXT-X-PROGRAM-DATE-TIME` for every segment and
`#EXT-X-ENDLIST`.

Track files (**observed**), one Ogg/Opus file per audio track:

| Track | Length | Expected tone | Level | Strongest other tone | End reason |
|---|---|---|---|---|---|
| alice microphone | 132.66 s | 440 Hz | -12.0 dBFS | -93.7 dBFS | StopEgress API |
| alice share audio | 132.84 s | 1000 Hz | -12.0 | -92.6 | StopEgress API |
| bob microphone | 133.74 s | 620 Hz | -12.0 | -85.5 | StopEgress API |
| bob share audio | 133.56 s | 1370 Hz | -12.1 | -96.0 | StopEgress API |
| carol microphone | 36.40 s | 790 Hz | -12.1 | -83.0 | Source closed |
| carol share audio | 31.90 s | 1730 Hz | -12.1 | -96.6 | Source closed |

**Start requests wait with Egress's default CPU costs (observed).** Egress
logs `cpu available: 6.000000 max cost: 4.000000` with the 6 CPU limit. With
the default settings the composite and the first track egress started at
once, and the second `StartTrackEgress` returned after 15.6 s, 14.3 s and
14.5 s (three calls). The track files of those calls begin 15 s after the
composite. With these values in the Egress configuration the start requests
of all later calls returned in 0.12 to 0.8 s, except one that took 3.0 s:

```yaml
cpu_cost:
  max_cpu_utilization: 0.8
  room_composite_cpu_cost: 3.0
  track_composite_cpu_cost: 0.5
  track_cpu_cost: 0.1
```

The measured use is 2.0 to 2.1 cores on average for a 1080p30 composite and
0.08 for a track egress (X7).

## X3. Surviving a recorder crash: measured

Method: a call with alice and bob, recordings start at 10 s, `docker kill
junto-spike-rec-egress` (SIGKILL) at 70 s. The time of the kill is the
container's `State.FinishedAt`. "Seconds lost" is that time minus the
wall-clock time of the last recorded sample, which comes from the last 10 s
time marker in the recovered sound. For share audio, which has no marker, it
is the kill time minus (reported start time + length).

Where Egress writes (**observed**): every egress writes into
`/home/egress/tmp/<egress id>/`. A file output is moved to the requested path
when the egress ends. A segment is copied to the requested path when the
segment is complete, and both playlists are written again each time. The
directory is fixed: setting `TMPDIR` to another path had no effect.

| Output | In the final location after the kill | In Egress's working directory | Plays | Seconds lost |
|---|---|---|---|---|
| (a) Composite, MP4 file (720p, two runs) | nothing | the partial `.mp4` (13.3 MB after 56 s) | no: `moov atom not found` | everything |
| (b) Composite, segments of 2 s, 1080p (`rth-kill-product`, `rth-rescue`) | 25 and 16 segments, the playlist without `#EXT-X-ENDLIST` | one unfinished segment | yes, after the playlist is closed | 4.16 and 4.49 |
| (b) Composite, segments of 2 s, 720p, two composites at once, load 13 to 32 (`rth-kill-var`) | 23 segments and the playlist | one unfinished segment | yes | 6.04 |
| (b) Composite, segments of 6 s, with an MP4 from the same egress (`rth-kill-var`) | 7 segments and the playlist; no MP4 | the partial `.mp4`, one unfinished segment | segments yes, MP4 no | 7.94 |
| (c) Track egress, Ogg file (four runs, 16 files) | nothing | the partial `.ogg` | yes: ffprobe reads it, ffmpeg decodes it without an error message | microphones 1.0 to 3.0 by marker in three runs; share audio 1.8 to 3.3 by start time |
| (d) Audio track as segments of 2 s (`StartTrackCompositeEgress`, AAC) | 28 segments and the playlist | one empty segment | yes | 1.42 |
| (d) Audio track as Ogg file through `StartTrackCompositeEgress` | nothing | the partial `.ogg` | yes | 2.24 |

- In the first run (`rth-kill-plain`) the kill time is known only to 1.2 s; its
  microphone files show 3.2 and 5.5 s lost. They are not in the range above.
- The composite's loss has three parts (**observed** in `rth-kill-product`):
  the delay of the composite's content (1.45 s, see X6), the segment that was
  being written (up to 2 s), and the time since the last copy (the newest
  segment was written 0.65 s before the kill).
- **The working directory does not survive a start of the container
  (observed).** The image's entrypoint runs `rm -rf /home/egress/tmp/*`. With
  the working directory inside the container, `docker cp` reads the partial
  files from the stopped container until it is started again. With a host
  directory mounted at `/home/egress/tmp`, the partial files were on the host
  after the kill (52 MB, most of it the profile directory of Chrome) and the
  directory was empty 5 s after `docker start`.
- **`data-host/egress-entrypoint.sh` (observed in `rth-rescue`).** With this
  script as the entrypoint and `restart: unless-stopped`, the Egress service
  process was killed inside the container (`kill -9`). The container ended,
  Docker started it again (`RestartCount` 1), the script copied four partial
  Ogg files, both playlists and the unfinished segment to
  `/out/rescued/<egress id>/`, and Egress answered on its health port 1.55 s
  after the kill. The four Ogg files decode without an error message.
- The runs with `docker kill` had `restart: "no"`. **Not verified:** whether
  Docker applies a restart policy after `docker kill`. The policy was applied
  after the kill inside the container (`RestartCount` became 1).

Turning the surviving segments into one MP4 (**observed**, five groups of
segments from three runs):

```sh
# 1. The playlist of a killed Egress has no end marker. Add it to a copy.
{ cat composite.m3u8; echo '#EXT-X-ENDLIST'; } > composite-closed.m3u8
# 2. Join without re-encoding.
ffmpeg -v error -nostdin -y -i composite-closed.m3u8 -c copy -movflags +faststart composite.mp4
```

- Without step 1, ffmpeg reads the playlist as a live stream and waits for new
  segments. It did not end in 25 s in 5 of 5 cases.
- With step 1 the command took 0.11 to 0.30 s and the result decodes without
  an error message (32 to 56 s long). In all five cases the playlist listed
  exactly the segments that were on disk.
- Without a playlist, `ffmpeg -f concat -safe 0 -i list.txt -c copy` over the
  segment files in name order also gave a playable MP4. In 2 of 5 cases its
  decoding printed 4 and 5 messages `non monotonically increasing dts`, and
  the result is 0.1 to 0.3 s longer. `scripts/recover_segments.sh` has both
  forms.
- **Not verified:** repairing the partial MP4 with another tool; a kill during
  the copy of a segment or of the playlist.

### Recommended configuration for the product

1. **Composite: `segment_outputs` only, `segment_duration` 2.** The server
   builds the MP4 with the ffmpeg command above after the recording ended, or
   after a crash from what is there.
2. **Audio tracks: track egress to an Ogg file, as before, with Egress's
   working directory on a mounted directory of the data volume and
   `egress-entrypoint.sh` as the entrypoint.** After a crash the partial Ogg
   files are in `/out/rescued/`.
3. `cpu_cost` as in X2, `restart: unless-stopped` for Egress.

Alternative for the audio tracks: `StartTrackCompositeEgress` with segments.
It lost less (1.42 s) and needs no rescue step, but it re-encodes to AAC and
writes 30 files per minute and track. It was run once.

Cost of this configuration (**observed**):

| | Value |
|---|---|
| CPU of Egress | No difference to MP4 output: 250 % (segments) and 264 % (MP4) for the same call on the quiet data host (X7). |
| Disk while recording | The segments are 6.0 to 6.5 % larger than the MP4 of the same encoder (46.1 MB and 43.4 MB for 132 s; seven calls). 30 files per minute. |
| Disk for the conversion | Segments and MP4 exist at the same time until the segments are deleted. |
| Key frame every 2 s | No increase measured: 14.1 MB per minute as segments and 17.0 MB per minute as MP4 for two parts of the same call (`rth-x7q`). The MP4 output alone had a key frame about every 9 s. |
| Delay before the final MP4 exists | 0.25 to 0.40 s for 130 s of 1080p. 4.2 s for a 61.5 minute file of 1.27 GB (made by repeating one recording 28 times; NVMe disk, files in the page cache). Plus the result check of X8. |
| Lost at a crash | Composite 4.2 to 4.5 s, audio tracks 1.0 to 3.0 s, then the time until the new recording records (X4). |

## X4. Detecting a lost recorder: PASS

Polled once per second after the kill (`rth-kill-plain`, `rth-kill-var`,
`rth-rescue`). **Observed:**

| Signal | What it reports | After how long |
|---|---|---|
| `ListEgress` (`room_name`, or `active: true`) | Every killed egress stays `EGRESS_ACTIVE`, `ended_at` 0, empty `error`, `updated_at` unchanged | No change in 95 s. `StopEgress` for them still answered 408 at 170 s. At the end of the test 23 such records were still listed as active. |
| `ListParticipants` | The recorder's participant (identity = egress id, kind `EGRESS`) is still listed, state `ACTIVE` | It is gone 19.4 to 21.6 s after the kill (three runs, 18 participants). In two runs some were gone about 1 s before the others. |
| `StopEgress` for a killed egress | HTTP 408 `{"code":"deadline_exceeded","msg":"request timed out"}` | 3.0 s after the request |
| Egress health port (`health_port: 9090`, answers `{"CpuLoad":...}`) | No answer | At once (0.16 s after `docker kill`) |
| Segments of the composite | No new file | The newest segment was 0.65 to 1.8 s old at the kill; with 2 s segments a file older than 6 s means that the recorder stopped |

A new egress's participant appears in `ListParticipants` 1 to 5 s after the
start request (track egress 1 to 2 s, composite 3 to 5 s).

Rule for the controller (derived from the table):

1. Every 5 s, for every recording that the controller believes is running,
   call `ListParticipants` for the room. An egress that was started more than
   10 s ago and has no participant with its identity is lost. This takes 20 to
   25 s after a crash.
2. The controller runs on the data host beside Egress, so it can decide
   earlier: the health port does not answer, or the newest file with the
   composite's segment prefix is older than three segment lengths. By the
   numbers of the table both are true at most 6 s after the kill. This rule
   itself was not run as code.
3. Then: mark the segment of every running egress of that Egress instance as
   failed with the saved time range, close the playlist and build the MP4
   from the surviving segments, take the partial Ogg files from `/out/rescued/`,
   and start a new composite and a new track egress for every audio track.
4. Do not use the status in `ListEgress` for this decision, and do not wait
   for `StopEgress`.

New recording in the same room (**observed**):

- After `docker kill` and `docker start` 95 s later, a new composite (MP4 and
  2 s segments) and four track egresses started in 0.1 to 0.8 s per request
  and ended `EGRESS_COMPLETE` with complete files (28 segments; two runs).
- With the restart policy (`rth-rescue`): Egress answered 1.55 s after the
  crash, the new composite was requested 2.25 s after it and its file begins
  6.33 s after it (content from about 4.7 s after it), 33 segments, `EGRESS_COMPLETE`.
  The composite recording has a gap of about 9 s (4.5 s lost before the crash
  and 4.7 s until the new content). The new track egresses were requested 3.1
  to 3.5 s after the crash.
- Between 3 s and 21 s after the crash the room contained the participants of
  the dead egresses and of the new ones at the same time.

## X5. Link loss: PASS

`call_link.py`: alice, bob and carol publish, a subscriber reports once per
second what it receives, a 1080p composite (MP4 and segments) and five track
egresses record. `docker stop junto-spike-rec-link` at 60 s, `docker start`
30.5 s later. Data host load 17 to 27 during this run. **Observed:**

| Question | Result |
|---|---|
| Do calls on the public host continue? | Yes. The subscriber received 96 to 101 audio frames, 14 to 16 camera frames and 4 to 5 share frames of every publishing participant in every second of the outage and after it (`out/rth-link/link-analysis.json`). |
| The running recordings | They continued. After the call every file is complete: composite MP4 and segments 153.3 s (77 segments), four track files 154.4 to 154.6 s. The check of X8 finds no hole in any of them. |
| `ListEgress` and `ListParticipants` during the outage | Answered normally (the API was called on the public address). All egresses `EGRESS_ACTIVE`, all recorder participants present. |
| `StopEgress` during the outage | HTTP 408 after 3.0 s. LiveKit logs `psrpc.EgressHandler.StopEgress client error ... no response from servers`. That egress kept recording and was stopped normally later. |
| A track that ends during the outage (carol left 15 s into it) | Egress closed the file at once (`End reason: Source closed`, complete, 62.0 s). `ListEgress` showed it as `EGRESS_ACTIVE` until 16 s after the link returned, which is 31 s after the file ended, then `EGRESS_COMPLETE` with the correct end time. |
| A start request during the outage (sent 12 s into it) | It did not fail. It returned HTTP 200 after 21.4 s, 3 s after the link returned, and the egress recorded. **Not verified:** a request that waits longer than the 22 s after which LiveKit answers 503 (seen in `../recorder`). |
| What Egress logs | Once `psrpc.IOInfo.UpdateEgress client error` and once `io connection unhealthy`, both 30 s after that track ended (15 s after the link returned). One second later `ListEgress` showed the change. No other warning from the outage. |
| What recovers by itself | The WireGuard handshake (0.61 s after the container started), Egress's connection to Redis, the delayed state update, and the waiting start request. No container was restarted and no request was repeated by the test. |

- A start request sent 20 s after the link returned took 0.15 s.
- The test stops the whole link container, which also ends the forwarder that
  Egress is connected to. **Not verified:** an outage of the network path
  while both containers keep running; an outage longer than 30 s; a restart of
  Redis.
- An Egress container that shares the network namespace of the link container
  was not tested. In that form Egress would lose its media path together with
  the link.

## X6. Timeline: PASS with a method

Nine composite recordings and 24 microphone track files with time markers:
`rth-x2` and `rth-x6a` to `c` on the loaded data host (load 4 to 22, CPU
pressure 13 to 17 % in `a` to `c`), `rth-x6d` to `f` and the two composites of
`rth-x7q` on the quiet data host (load 1 to 9, CPU pressure 2.6 to 5.2 %).
"Late" is: reported start time + position of a marker in the file - true time
of the marker. All files are 1080p30. **Observed:**

| Recording | Data host | Composite sound late (median, range) | Composite picture late | Composite media longer than `ended_at - started_at` | Late minus longer | Track files late |
|---|---|---|---|---|---|---|
| `rth-x2` | loaded | 1.569 s (1.561 to 1.599) | 1.460 s | 1.440 s | 0.129 s | 41, 41, 43 ms |
| `rth-x6a` | loaded | 1.961 s (1.946 to 1.975) | 1.768 s | 1.824 s | 0.137 s | 42, 41, 53 ms |
| `rth-x6b` | loaded | 1.637 s (1.598 to 1.899) | 1.606 s | 1.476 s | 0.161 s | 44, 41, 56 ms |
| `rth-x6c` | loaded | 1.950 s (1.930 to 2.000) | 1.865 s | 1.812 s | 0.138 s | 44, 42, 50 ms |
| `rth-x6d` | quiet | 1.518 s (1.493 to 1.579) | 1.413 s | 1.385 s | 0.133 s | 46, 53, 42 ms |
| `rth-x6e` | quiet | 1.549 s (1.525 to 1.619) | 1.412 s | 1.422 s | 0.127 s | 46, 49, 46 ms |
| `rth-x6f` | quiet | 1.691 s (1.678 to 1.711) | 1.554 s | 1.571 s | 0.120 s | 44, 41, 43 ms |
| `rth-x7q`, segments only | quiet | 1.492 s (1.486 to 1.494) | 1.369 s | 1.366 s | 0.126 s | 49, 42, 43 ms |
| `rth-x7q`, MP4 only | quiet | 1.504 s (1.493 to 1.544) | 1.399 s | 1.372 s | 0.132 s | (the same three files) |

- **Track files:** every one of the 24 files is 41 to 56 ms late against
  `file_results.started_at`, and inside one file all markers agree within
  0.3 ms. After subtracting 47 ms the error is -6 to +9 ms.
- **Composite:** the content of the file begins before the reported start
  time. An event at wall-clock time T is in the file at `T - started_at + D`,
  with D between 1.49 and 1.96 s. D differs from recording to recording on the
  quiet host too (1.49 to 1.69 s). A fixed value of D therefore gives an error
  of up to 0.3 s.
- **D can change inside a recording, and differ between participants
  (observed once, `rth-x6b`, loaded host).** For the first 40 s the sound of
  alice and bob was 1.60 s late. When carol joined, two markers of alice were
  not found and one of bob was measured at -3.26 s (not examined; counted as a
  wrong detection). Then bob's sound was 2.40 s late and alice's 1.90 s, and
  both returned to 1.63 s over the next 60 s, while carol's sound was 1.61 to
  1.79 s late in the same seconds. The file's timestamps have no hole. The
  page plays every track through its own buffer in Chrome, so the tracks of
  one composite are not always equally late.
- For the MP4 and the segments of one egress the values are equal within
  1 ms. `segment_results.started_at` equals `file_results.started_at`, and the
  first `#EXT-X-PROGRAM-DATE-TIME` of the playlist is 1 to 14 ms before it.
- Inside the composite the picture is 0.03 to 0.19 s ahead of the sound (median
  per recording). The video marker is exact to one camera frame (67 ms).

Methods to place the composite on the timeline of the track files:

| Method | Error in the 9 recordings |
|---|---|
| A. `started_at` as reported | 1.49 to 1.96 s |
| B. `started_at` minus a fixed 1.65 s | up to 0.16 s in one direction and 0.31 s in the other |
| C. By the end: first sample at `ended_at - (media length from ffprobe) - 0.135 s` | -15 to +26 ms |
| D. By the sound: shift at which the loudness envelope of the composite's sound agrees best with one microphone track file (`scripts/align.py`, 60 s of sound, 2 ms steps) | -38 to +4 ms against the track file; +38 to +81 ms on the wall clock, because it includes the 41 to 56 ms of the track file |
| D per 20 s window | -37 to +16 ms in 43 of 45 windows; -79 ms and no usable value in the two windows of `rth-x6b` in which D changed |

**Method for the product:** place every track file at
`file_results.started_at - 0.047 s`. Place the composite with method C. For a
composite without `ended_at` (a killed recorder), use method D against the
partial track file. With these, all files of the nine recordings are on one
timeline within 30 ms, except for a change of D inside a recording
(`rth-x6b`, up to 0.3 s for part of the file), which only method D per window
follows.

**Not verified:** method D with speech (the test sound is tones with a gap
every 10 s); recordings longer than 3 minutes and drift over an hour; the
constants 0.047 s and 0.135 s with another Egress version or another machine.

## X7. Cost: measured

### One recording (`x7_cost.py`)

A call with three publishing participants (five audio tracks, two shares,
three cameras) and two subscribers. Phase A: no recording. Phase B: 1080p30
composite as 2 s segments and five track egresses. Phase C: the same with the
composite as one MP4. The public host was sampled every 5 s
(`public-host/sample.sh`: CPU time of the LiveKit container from its cgroup,
percent of the one core). Egress was sampled with `docker stats` every 2 s.
**Observed:**

| Run | Phase | Egress CPU mean / highest sample | Egress memory | LiveKit CPU on the public host, mean / highest sample | Public host busy (steal) | Public host sends |
|---|---|---|---|---|---|---|
| quiet data host (`rth-x7q`, load 1 to 5) | A no recording | | | 11.9 % / 28 % | 19 % (13 %) | 2.6 Mbit/s |
| | B segments + 5 tracks | 250 % / 293 % | 1.36 GiB | 9.4 % / 13 % | 13 % (5 %) | 4.0 Mbit/s |
| | C MP4 + 5 tracks | 264 % / 478 % | 1.42 GiB | 10.3 % / 18 % | 16 % (8 %) | 4.0 Mbit/s |
| loaded data host (`rth-x7`, load 14 to 30, CPU pressure 32 to 36 %) | A no recording | | | 14.1 % / 31 % | 24 % (16 %) | 2.7 Mbit/s |
| | B segments + 5 tracks | 390 % / 544 % | 1.55 GiB | 12.5 % / 19 % | 20 % (11 %) | 4.0 Mbit/s |
| | C MP4 + 5 tracks | 412 % / 588 % | 1.48 GiB | 13.4 % / 25 % | 21 % (12 %) | 3.9 Mbit/s |

- Egress's own figures per egress (its `egress metrics` log line, quiet run):
  composite as segments 2.01 cores on average and 3.01 at peak; composite as
  MP4 2.13 and 3.73; a track egress 0.08 on average, 0.17 at peak and 88 MB.
- The same recording cost 1.5 times the CPU time when other work used the data
  host. In that run Egress reached its limit of 6 CPUs in single samples. Both
  recordings pass the check of X8.
- **The recording did not raise the CPU of LiveKit on the public host.** The
  recorder adds six subscribers and 1.3 Mbit/s of sent data. The differences
  between the phases (-2.5 and -1.6 points) are smaller than the variation of
  the hypervisor's steal time (5 to 16 %).
- The calls of X6 with MP4 and segments from one egress and four to six track
  egresses: 253 to 267 % on the quiet host, 320 to 325 % on the loaded host.

### Two load points with a recording (`x7_load.py`)

The load ran from containers on the data host (`lk load-test`, limited to 4
CPUs). The recording is one 1080p30 composite as 2 s segments, started 12 s
after the load. The averages cover the time in which the recording was
active (39 to 49 s), or the same part of the run without a recording
(54 s).
**Observed:**

| Load point | Recording | LiveKit CPU mean / highest sample | Host busy (steal) | Host receives / sends | UDP buffer errors | Loss reported by the generator | Egress CPU mean |
|---|---|---|---|---|---|---|---|
| One 2.5 Mbit/s stream, 40 subscribers | none | 23.0 % / 43 % | 27 % (10 %) | 3 / 121 Mbit/s | 252 | 1.25 % | |
| | none (second run) | 28.0 % / 32 % | 33 % (16 %) | 3 / 103 Mbit/s | 251 | 4.75 % | |
| | yes | 28.0 % / 55 % | 40 % (15 %) | 3 / 109 Mbit/s | 287 | 3.14 % | 184 % |
| | yes (second run) | 25.7 % / 36 % | 30 % (12 %) | 3 / 120 Mbit/s | 181 | 1.23 % | 177 % |
| 16 participants publish simulcast video, 16 subscribers, 4x4 | none | 28.8 % / 57 % | 34 % (15 %) | 7 / 85 Mbit/s | 550 | 4.70 % | |
| | yes, layout page as it is | 31.9 % / 44 % | 37 % (9 %) | 48 / 71 Mbit/s | 877 | 2.76 % | 545 % (the 6 CPU limit) |
| | yes, layout flag `lowtiles` | 17.3 % / 23 % | 21 % (6 %) | 4 / 49 Mbit/s | 3 | 1.06 % | 261 % |

- **40 subscribers:** 23.0 and 28.0 % without a recording, 25.7 and 28.0 %
  with one. The recording made no difference that is larger than the
  difference between two runs of the same kind.
- **The values of this test without a recording (23 to 28 % and 29 %) are
  higher than the earlier 14 % and 23 %.** The steal time was 10 to 16 % in
  these runs, and the generator reported 1.2 to 4.8 % loss (earlier 0.12 % and
  2.36 %). The place of the loss was not determined. The values with a
  recording must be compared with the values of this test without one.
- **16 publishers:** the layout page subscribes to every camera at full
  quality. With the recording the public host received 48 Mbit/s instead of
  7 Mbit/s, because the publishers then send their 720p layers, and Egress used
  all 6 CPUs and 2.3 GiB. With the flag `lowtiles` (the page asks for the
  lowest layer of every tile) the host received 4 Mbit/s and Egress used
  261 %. In that run the subscribers also received less (49 instead of 85
  Mbit/s sent by the host), so its 17.3 % is not a comparison with the 28.8 %
  of the run without a recording. Each of the three runs was made once.
- **Not verified:** what the composite of the 16-camera call looks like (the
  page was made for a few tiles); the load points with track egresses.

Traffic of the whole test: 8.0 GB in the nine sampled runs (**observed** from
the public host's interface counters), and an estimated 5 GB for image
downloads and the other calls, about 13 GB in total.

## X8. Silent failure check: PASS

`scripts/check_recording.py FILE [--reference TRACKFILE:OFFSET ...]` does
three things:

1. **Packet timestamps** (`ffprobe -show_entries packet=...`, no decoding):
   every stream exists, begins within 1 s of the file's beginning, ends within
   1 s of its end, has no hole (sound: more than 0.25 s between packets;
   picture: more than 1 s), and the picture has at least 90 % of the expected
   frames.
2. **Sound** (audio decoding only, `silencedetect=noise=-70dB:d=1` and
   `volumedetect`): the file fails when its highest level is below -60 dB.
   With `--reference`, a range that is silent in the file while a track file
   has sound at the same time is counted as missing sound.
3. **Picture** (`-skip_frame nokey`, key frames only, scaled to 160x90,
   `signalstats`): the file fails when more than 5 % of the key frames are
   dark or have one colour.

Test files from `call_x8.py` (720p composites of 36 s) and the result
(**observed**):

| File | How it was made | What Egress reported | Check |
|---|---|---|---|
| good | normal layout page | `EGRESS_COMPLETE`, no error | ok |
| nosound | the page attaches no audio track | `EGRESS_COMPLETE`, no error | fails: `sound is digital silence for the whole file` (-91 dB), and 36.8 s silent while the reference has sound |
| novideo | the page hides the stage | `EGRESS_COMPLETE`, no error | fails: 4 of 4 key frames have one colour |
| starved | `docker update --cpus 0.5` on the Egress container for 25 s of the recording | `EGRESS_COMPLETE`, no error | fails: 6 holes in the sound's timestamps, 21.7 s in total, the first at 5.9 s. The file is 32.2 s long for 40 s of recording. |
| damaged-audio-hole | audio packets of 8 s removed from the good file | | fails: 1 hole, 8.0 s, at 10.0 s |
| damaged-audio-muted | 8 s of the good file's sound set to zero | | fails: 8.0 s without sound while the reference has sound |
| damaged-video-short | the picture ends after 15 s | | fails: picture ends 21.3 s before the file, 12.4 frames per second |

- The check passed every good recording it was run on: the composites of X5,
  X6 and X7 (MP4 and segment playlists) and all track files of X5.
- **The earlier silent failure is reproduced (observed).** With 0.5 CPU,
  Egress logged `Can't record audio fast enough` 12 times and reported no
  error. The four track egresses that ran in the same container at that time
  have no hole.
- Cost of the check: 3.3 to 4.0 s for a 130 s 1080p file. For a 61.5 minute
  1080p file: 79.8 s in total, of which 2.9 s for the timestamps, 5.9 s for the
  sound and 71 s for 1848 key frames. For a track file of 155 s: 1.0 s.
- The controller has more signals without reading the file: the media length
  against the wall-clock length of the recording (32 s against 40 s in the
  starved file), and the line `Can't record audio fast enough` in the Egress
  log.
- **Not verified:** a recording with sound that is present but wrong (for
  example the sound of a share that is not the main one); the limits of the
  check with real content, where silence and a still picture are normal. The
  check treats only digital silence and only one-colour or dark key frames as
  defects, and the comparison with the track files decides for sound.

## Other observations

1. **Third-party STUN servers (observed again).** The Egress log has warnings
   that name public STUN servers of a third party. `../recorder` reported the
   same.
2. **Egress's debug log contains the recorder's room token (observed again).**
   The harness removes tokens and addresses before it saves log lines.
3. **`buffer full, dropping sample` (observed).** Egress logged this warning
   for a share-audio track egress once or twice per call in three calls. The
   files of the one call that was checked have no hole. The effect on the
   sound was not examined.
4. **A recorder participant is in the room for 20 s after its Egress died
   (X4).** The admission check of ADR 0506 must skip participants of kind
   `EGRESS`, including these.
5. **Egress accepts `health_port`.** `GET /` on it answers `{"CpuLoad":6}`.

## What the product's recording controller and deployment must do

Deployment:

1. **Run the link as a userspace WireGuard process in a container on each
   host, or use the kernel module as a decision of the administrator.** A container
   that calls `wg` for an interface before the userspace process is ready
   loads the kernel module. Wait for the control socket first
   (`link/entrypoint.sh`).
2. **Do not rely on `ufw` for a port that Docker publishes.** The port was
   reachable without a rule. Redis must stay unpublished or published on the
   loopback address, as it is now. LiveKit's metrics port 6789 listens on all
   addresses (reported by `../media-host`).
3. **Give Redis to the data host through a forwarder that listens on the
   link address only**, or let Redis listen on the link address. Keep the
   password. The link needs one allowed address per side and no IP forwarding.
4. **Keep Egress's media path independent of the link.** Egress connects to
   the public address of LiveKit and to a forwarder for Redis. Then a link
   outage of 30 s does not interrupt a recording. Do not put Egress into the
   network namespace of the link container.
5. **Mount a directory of the data volume at `/home/egress/tmp`, use
   `egress-entrypoint.sh` as the entrypoint, and set `restart:
   unless-stopped`.** Without the script the image deletes the partial files
   at every start.
6. **Set `cpu_cost` in the Egress configuration** (values in X2), or the
   audio files of a recording start up to 15 s after the composite.
7. **Reserve CPU for Egress on the data host.** One 1080p30 composite with
   five track egresses used 2.5 cores on average and 2.9 to 4.8 at peak on the
   quiet host, and 3.9 cores on average when other work used the host. The
   transcription worker of the same host must not take these cores during a
   recording.
8. **Keep the Egress log level above debug**, or keep the log private.

Recording controller:

9. **Record the composite as 2 s segments and build the MP4 with ffmpeg after
   the recording.** Add `#EXT-X-ENDLIST` to the playlist when Egress did not
   write it. Delete the segments after the MP4 passed the check.
10. **Record every audio track with a track egress to an Ogg file**, one new
    egress for every new track (as `../recorder` describes).
11. **Detect a lost recorder with `ListParticipants` and with local signals,
    not with `ListEgress`** (rule in X4). Mark the segment as failed with the
    saved range, recover what is on disk, start new egresses. Expect a gap of
    about 9 s in the composite and about 6 s in the audio tracks.
12. **Treat HTTP 408 from `StopEgress` as "not delivered".** It happens for a
    dead recorder and during a link outage. Decide which case it is with rule
    11, and repeat the stop when the recorder is alive.
13. **Expect delayed state after a link outage.** An egress that ended during
    the outage was reported as active until 16 s after the link returned. Take
    the end time from `file_results.ended_at`, not from the time the change
    was seen.
14. **Build the timeline as in X6:** track files at `file_results.started_at -
    0.047 s`, the composite at `ended_at - media length - 0.135 s`, and by the
    sound when there is no `ended_at`. Measure both constants again when the
    Egress version changes.
15. **Check every finished file** with the three passes of X8 and compare the
    composite's sound with the track files. Compare the media length with the
    wall-clock length. A failed check makes the recording "partly failed".
16. **Ask the layout page for the lowest layer of every tile.** With the
    current page a recording of a 16-camera call made every publisher send its
    highest layer.

## Not verified

- LiveKit API calls, signalling or media through the link. The API was called
  on the public address.
- TLS. Egress used `ws://` with `insecure: true`.
- A link outage longer than 30 s, an outage of the path without stopping the
  container, a Redis restart, a LiveKit restart in this arrangement.
- Recordings longer than 3 minutes. The one-hour figures for the conversion
  and the check come from a file made by repeating one recording.
- Real screen content and speech. Every source was a generated pattern or
  tone.
- The kernel implementation of WireGuard and the link's throughput.
- Whether the data host's own collection of container logs stored the logs of
  this test's containers.
- The storage guard, webhooks, and more than one Egress instance.

## Changes on the two machines, and their removal

### Public host

Added during the test:

| Item | Removed |
|---|---|
| Directory `/opt/junto-spike-link/`: `compose.yaml`, `.env`, `link/`, `keys/private`, `sample.sh`, `unload.sh`, `v6addr`, `orig/compose.yaml` and `orig/livekit.yaml` (copies of the existing stack's two files) | `rm -rf /opt/junto-spike-link`; `ls /opt` shows `containerd` and `junto-spike` |
| Container `junto-spike-link` and image `junto-spike-link` | `docker compose down`, `docker rmi`; `docker ps -a` shows the four containers that ran before |
| Short-lived containers `junto-spike-link-keygen` and `junto-spike-link-ver` (`--rm`) | gone when they ended |
| Firewall rule `ufw allow 51820/udp comment junto-spike-link` (entries `[ 7] 51820/udp` and `[14] 51820/udp (v6)`). ufw wrote its files `/etc/ufw/user.rules` and `user6.rules` again for it. | `ufw delete allow 51820/udp` answered `Rule deleted` and `Rule deleted (v6)`; `ufw status numbered` shows the 12 entries that existed before, and the two files contain no line with `51820` |
| Build cache of the two image builds, 26.1 MB (all 9 records were from this test) | `docker builder prune -af`; `docker buildx du` shows 0 B |
| Kernel module `wireguard` and 8 modules it needs, loaded without intent (see "Deviation") | `rmmod`; `lsmod` shows none of them |

Not changed (**observed**): `/opt/junto-spike/compose.yaml` and `livekit.yaml`
have the same SHA-256 sums at the end as at the start, and equal the copies
that were kept. The containers `junto-spike-livekit` and `junto-spike-redis`
and the two containers of an unrelated service on the host were not stopped,
restarted or changed; `docker ps` shows their uptimes continuing through the
test (4 hours, 3 hours, 5 months, 5 months at the end). No package was
installed.
No rule other than the one above was added or deleted.

### Data host

Added during the test:

| Item | Removed |
|---|---|
| Directory `/srv/junto-spike-rec/` (sources, `.env`, `keys/private`, build directory, recordings, logs) | `rm -rf`; `ls` answers `No such file or directory` |
| Containers `junto-spike-rec-link`, `-layout`, `-egress`, `-ctl` and the Compose network | `docker compose down`; 0 containers and 0 networks with the prefix |
| Short-lived containers `junto-spike-rec-probe`, `-keygen`, `-loadpub`, `-loadgen` | gone when they ended or removed by the script |
| Images `junto-spike-rec-link`, `-layout`, `-tools`, `livekit/egress:v1.15.0`, `livekit/livekit-cli:v2.18.8`, `redis:7.2.16-alpine` | `docker rmi`; 0 images of these names |
| Build cache of this test's image builds (about 3.6 GB with the base images that these builds downloaded) | `docker buildx prune --filter id=...` for each record; see "what remains" for one record |
| Kernel module `wireguard` and 3 modules it needs, loaded without intent (see "Deviation") | `rmmod`; `lsmod` shows none of them |

Limits used: Egress `--cpus 6 --memory 8g`; tools container 4 CPUs and 6 GB;
load generator 4 CPUs and 4 GB; link and layout page 0.5 CPU and 128 MB each.
No host package or host configuration was changed, no existing container or
service was stopped or restarted, no firewall or host network setting was
changed. The outputs of the removal are in `out/cleanup-*.txt`.

## What remains after the test

**Public host:**

- The original stack, running: `junto-spike-livekit` and `junto-spike-redis`,
  the files in `/opt/junto-spike/`, the six `ufw` entries with the comment
  `junto-spike`. A test join worked after everything of this test was removed:
  from the data host (a publisher and a subscriber; the subscriber found the
  440 Hz tone, 0 packets lost) and from the Mac (`lk room join`: `connected to
  room`, `ListRooms` shows 1 participant).
- In the stack's Redis: 126 egress records of rooms named `rth-*`. 103 are
  `EGRESS_COMPLETE`. 23 are `EGRESS_ACTIVE` and belong to recorders that the
  test killed (rooms `rth-kill-plain`, `rth-kill-var`, `rth-kill-product`,
  `rth-rescue`); `ListEgress` with `active: true` returns them. They were not
  deleted, because that means writing into LiveKit's data. This Redis keeps
  nothing on disk, so they disappear when it restarts.
- Lines about this test in the container log of `junto-spike-livekit` and in
  the kernel log (the module load).
- Nothing else of this test.

**Data host:**

- One build cache record of 3.79 MB, created by the build of the link image.
  Docker marks it as shared (it is the base layer of the `alpine:3.22` image
  that was on the machine before the test) and does not remove it by its
  identifier.
- Two kernel log lines about the module load at 02:11:04.
- Nothing else of this test: no container, image, network, volume, file or
  loaded module.

**Mac:** this directory with its `out/` (14 MB, not tracked). Nothing was
built on the Mac. The three `JUNTO_TEST_LK_*` lines of `.env.local` were read
and not changed.
