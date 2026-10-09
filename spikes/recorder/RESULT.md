# Recorder spike: result

Date of the test: 2026-10-09.

## Question

Can LiveKit Egress produce what the design's recording needs?

1. One composite MP4 per recording: the most recently started screen share as
   the main picture, cameras and other shares as tiles, and as sound the main
   share's audio plus every microphone, without the audio of other shares.
2. One audio file per audio track (each microphone, each share-audio track),
   with a new segment after a reconnect and a known start time per segment.
3. A job state that the server can recover by asking the recorder.

## Result per gate

| Gate | Result | Short statement |
|---|---|---|
| R1 custom layout page | PASS | Picture and sound follow the rule, and both switch when a third share starts. The page must not subscribe to the audio of a non-main share (see R1). |
| R2 separate audio files | PASS | One Ogg/Opus file per track with the right tone. Egress transcodes: every file is re-encoded to stereo Opus at about 128 kbit/s. |
| R3 reconnect and mute | PASS | Reconnect: the file ends complete, the controller must start a new egress for the new track. Mute: silence inside the same file. Track files are within 37 ms of the wall clock. The composite MP4 is about 1.0 s late against its reported start time. |
| R4 state recovery | FAIL for an Egress restart, PASS for a LiveKit restart | Egress restart or crash: the running recordings and their partial files are lost, and `ListEgress` keeps reporting them as `EGRESS_ACTIVE` (observed for 35 minutes). LiveKit restart: nothing is lost except the seconds of the restart. |
| R5 limits | measured | 1 composite + 30 track egresses ran at once (none refused). 5 composites ran at once; the 6th request got no answer in 20 s. 720p30 composite + 4 track egresses: 0.94 CPU core on average, 2.2 at peak, 0.65 GiB. |
| R6 control by API | PASS | Start and stop by API with a custom template URL work. The main share can be named in the start request, by `UpdateLayout`, by room metadata and by a data message; all four were observed. |

Every statement below is marked **observed** (it was run and seen in this
test) or **not verified**.

## Versions and environment

| Component | Version |
|---|---|
| LiveKit server | `livekit/livekit-server:v1.13.9` (linux/arm64) |
| LiveKit Egress | `livekit/egress:v1.15.0` (linux/arm64); it contains Chrome 154.0.8037.57 and GStreamer 1.24.13 |
| Redis | `redis:7-alpine` |
| Layout page server | `nginx:alpine` with `livekit-client` 2.22.3 (UMD build) |
| LiveKit Rust SDK (test participant) | `livekit` 0.9.4, `livekit-api` 0.8.2, `libwebrtc` 0.3.51 |
| `lk` command-line tool | 2.18.8 (Homebrew formula `livekit-cli`) |
| Docker | Docker Desktop, engine 29.2.1, linux/arm64, 8 CPUs and 7.65 GiB for the VM |
| Host | macOS on arm64, 8 cores |
| Analysis | ffmpeg and ffprobe 9.0.1, Python 3.14 with numpy |

**Egress on arm64 (observed):** the `livekit/egress` image has a linux/arm64
variant, and it ran natively, including Chrome for the composite recording. No
emulation was used. `docker manifest inspect` lists amd64 and arm64 variants
for `livekit/livekit-server`, `livekit/egress` and `redis:7-alpine`.

**Load on the test machine (observed):** other work unrelated to this spike
used the CPU during every run. The 1-minute load average of the host was
between 8 and 18 during the runs reported here, on 8 cores. The CPU figures
and the audio drop counts below are from that condition. A first attempt of
the main call ran while the load average reached 88, and its composite MP4
was damaged (see "Other observations", item 1); that attempt is not used for
the gates.

## Method

Files of this spike:

| Path | Content |
|---|---|
| `compose.yaml`, `livekit.yaml` | The stack: LiveKit server, Redis, Egress and the layout page server. Container names start with `junto-spike-rec-`. |
| `layout/` | The custom layout page (`index.html`, `layout.js`) and the Dockerfile of its server. |
| `scripts/stack.sh` | Creates `.env.local` (generated API key and secret, not tracked) and runs `docker compose`. |
| `scripts/lkapi.py` | Calls the LiveKit server API (Twirp over HTTP) and prints the JSON answer. |
| `scripts/harness.py` | The test controller: starts participants, starts a track egress for every new audio track, logs every change in `ListEgress`, samples `docker stats`. |
| `scripts/call_a.py`, `analyze_call_a.py`, `show_analysis.py` | The main call (R1, R2, R3, R6, part of R5) and its analysis. |
| `scripts/call_b.py` | Restart tests (R4). |
| `scripts/limits.py`, `stats_1080p.py` | R5. |
| `scripts/probe.py`, `probe2.py`, `probe3.py` | Smaller checks: the unified `StartEgress` request, pass-through, graceful stop, share order, refusal, liveness of a lost egress. |
| `scripts/tones.py` | Tone levels and tone edges of a media file. |
| `scripts/show_log.py` | Prints a controller log as text. |
| `requests/` | Request bodies for the `lk` tool. |
| `../test-participant/` | The synthetic participant (Rust). |

Participants are processes of `../test-participant`. Each one publishes a
microphone tone, a camera pattern, a screen-share pattern and a share-audio
tone. Tones are at -12 dBFS:

| Participant | Microphone | Share audio | Screen colour |
|---|---|---|---|
| alice | 440 Hz | 1000 Hz | blue |
| bob | 620 Hz | 1370 Hz | red |
| carol | 790 Hz | 1730 Hz | green |

Tone measurement: the audio of a file is decoded with ffmpeg and cut into
windows of 0.1 s. The level of each tone frequency is measured per window
(Hann window, single-frequency transform). A tone is **present** in a period
when its median level is above -20 dBFS, and **absent** when its median is
below -60 dBFS. The test participant does not use the SDK's platform audio
mode, so it opens no audio device of the machine (from the SDK's
documentation; not measured). Egress plays the page's sound into a virtual
sink inside its container.

Time marker: at every Unix-time multiple of 10 s, for 300 ms, each microphone
tone is silent and the lower third of each video pattern is white. The
position of a marker in a file plus the file's reported start time gives a
wall-clock time, and the difference to the marker's true time is the
placement error of that file.

## Commands

Stack:

```sh
cd spikes/recorder
./scripts/stack.sh up -d --build      # creates .env.local on first use
./scripts/stack.sh ps
./scripts/stack.sh down -v
```

Test participant:

```sh
cd spikes/test-participant
../../tools/serial.sh cargo build --release
../../tools/serial.sh cargo test --release      # 2 tests passed
```

Test runs, from `spikes/recorder`. Each writes to `out/<room>/`:

```sh
../../tools/serial.sh python3 scripts/call_a.py call-a
python3 scripts/analyze_call_a.py call-a > out/call-a/analysis.json
python3 scripts/show_analysis.py call-a
python3 scripts/show_log.py call-a

../../tools/serial.sh python3 scripts/call_b.py b-egress-restart egress-restart
../../tools/serial.sh python3 scripts/call_b.py b-egress-kill egress-kill
../../tools/serial.sh python3 scripts/call_b.py b-egress-term egress-term
../../tools/serial.sh python3 scripts/call_b.py b-livekit-restart livekit-restart

../../tools/serial.sh python3 scripts/limits.py limits
../../tools/serial.sh python3 scripts/stats_1080p.py stats-1080p
../../tools/serial.sh python3 scripts/probe.py probe
../../tools/serial.sh python3 scripts/probe2.py probe2
../../tools/serial.sh python3 scripts/probe3.py probe3
```

The same API calls with the `lk` tool (each was run once and worked):

```sh
set -a; . ./.env.local; set +a          # LIVEKIT_URL, LIVEKIT_API_KEY, LIVEKIT_API_SECRET

sed 's/ROOM/demo/g' requests/composite.json > out/composite.json
lk egress start --type room-composite out/composite.json
lk egress start --type track out/track.json        # requests/track.json with ROOM and TRACK_ID filled in
lk egress list --room demo --json
lk egress list --room demo --active
lk egress update-layout --id <egress id> --layout "main:alice;debug"
lk room update --metadata '{"main_share":"alice"}' demo
lk room send-data --room demo --topic layout '{"main_share":"alice"}'
lk room participants list demo
lk egress stop --id <egress id>
```

The API through the script, with the raw JSON answer:

```sh
python3 scripts/lkapi.py Egress ListEgress '{"room_name":"demo"}'
python3 scripts/lkapi.py Egress ListEgress '{"active":true}'
python3 scripts/lkapi.py RoomService ListParticipants '{"room":"demo"}'
python3 scripts/lkapi.py Egress StopEgress '{"egress_id":"EG_..."}'
```

Request bodies that the controller sent (`ROOM` is the room name):

```json
StartRoomCompositeEgress
{"room_name": "ROOM", "layout": "debug", "custom_base_url": "http://layout/",
 "preset": "H264_720P_30",
 "file_outputs": [{"file_type": "MP4", "filepath": "/out/ROOM/composite-{time}.mp4"}]}

StartTrackEgress
{"room_name": "ROOM", "track_id": "TR_...",
 "file": {"filepath": "/out/ROOM/tracks/alice-microphone-TR_...-{time}"}}

StartEgress (the unified request), composite
{"room_name": "ROOM",
 "template": {"layout": "main:alice;debug", "custom_base_url": "http://layout/"},
 "preset": "H264_720P_30",
 "outputs": [{"file": {"file_type": "MP4", "filepath": "/out/ROOM/unified-composite-{time}.mp4"}}]}

StartEgress (the unified request), one audio track
{"room_name": "ROOM",
 "media": {"audio": {"routes": [{"track_id": "TR_..."}]}},
 "preset": "PASSTHROUGH",
 "outputs": [{"file": {"filepath": "/out/ROOM/passthrough-mic-{time}"}}]}

UpdateLayout           {"egress_id": "EG_...", "layout": "main:alice;debug"}
UpdateRoomMetadata     {"room": "ROOM", "metadata": "{\"main_share\": \"bob\"}"}
SendData               {"room": "ROOM", "data": "<base64 of {\"main_share\": \"alice\"}>", "kind": "RELIABLE", "topic": "layout"}
StopEgress             {"egress_id": "EG_..."}
```

## The main call (`call-a`)

Times are seconds from the start of the run.

| Time | Event |
|---|---|
| 0 | alice joins and shares. bob joins. |
| 4 | bob starts his share. |
| 12 | The controller starts the composite recording (layout `debug`, no main share named) and one track egress per audio track (4). |
| 70 | carol joins. |
| 75 | carol starts her share. The controller starts 2 more track egresses. |
| 105 | `UpdateLayout` with `main:alice;debug`. |
| 120 | `UpdateRoomMetadata` with `{"main_share":"bob"}`. |
| 135 | `SendData`, topic `layout`, `{"main_share":"alice"}`. |
| 150 | The instruction is cleared (data message with an empty id, empty room metadata, layout `debug`). |
| 165 | bob disconnects, is away for 3 s, and joins again with new tracks. The controller starts 2 more track egresses. |
| 195 to 215 | alice's microphone is muted. |
| 245 | The controller stops all egresses. |

Result files: one MP4 (229 s) and eight Ogg files.

## R1. Custom layout page: PASS

The page is `layout/index.html` with `layout/layout.js`: plain HTML and
JavaScript with the LiveKit JS SDK. The container `junto-spike-rec-layout`
serves it, and the start request names it in `custom_base_url`. Egress opens
`<custom_base_url>?layout=<layout>&url=<server URL>&token=<token>` in Chrome
and starts recording when the page writes `START_RECORDING` to the console.

Sound, measured in the composite MP4 (**observed**). Each cell is the median
level in dBFS; a present tone is near -12, an absent tone is below -60:

| Period in the MP4 | Expected main share | 440 alice mic | 620 bob mic | 790 carol mic | 1000 alice share | 1370 bob share | 1730 carol share |
|---|---|---|---|---|---|---|---|
| 3 to 57 s: two shares, no instruction | bob (started last) | -12.0 | -12.0 | -81.5 (not in the call yet) | **-87.9** | **-12.0** | -88.4 |
| 62 to 88 s: after carol's share started | carol | -12.1 | -12.1 | -12.1 | -80.6 | -78.6 | **-12.1** |

In the first period the main share's tone (1370 Hz) is present, the other
share's tone (1000 Hz) is absent, and both microphone tones are present. After
the third share started, the sound changed to that share's tone (1730 Hz),
both other share tones are absent, and all three microphone tones are present.
The 95th percentile of the absent tones is between -48 and -82 dBFS; the
highest values are the clicks at the edges of the time markers, which contain
every frequency.

Picture (**observed**, frames extracted from the MP4 and viewed; they are in
`out/call-a/frames/`):

- Period 1: the main picture is bob's screen pattern (red, label
  `SCREEN BOB`). The tile column on the right holds bob's camera, alice's
  camera and alice's share (blue).
- Period 2: the main picture is carol's screen pattern (green, label
  `SCREEN CAROL`). The tiles are the three cameras and the shares of alice
  and bob.
- The status line that the page draws with the `debug` layout flag shows
  `main=bob by=auto` and then `main=carol by=auto`, with the list of tracks
  that the page plays.

Which share is "most recently started" when the page joins after the shares
(**observed**): the page orders shares by the time in the server's version
stamp of each track (`trackInfo.version` in the JS SDK), and falls back to the
time at which it first saw the track. In `call-a` bob shared 4 s after alice
and the page chose bob. In `probe2` alice shared 5 s after bob and the page
chose alice (1000 Hz at -12.1 dBFS, 1370 Hz at -88.3 dBFS). **Not verified:**
whether the version stamp changes when a share track is muted or updated; the
controller should name the main share instead of relying on this order.

Required property of the page (**observed**): the first version of the page
subscribed to every track and attached an audio element only for the tracks
it wanted to play. After the main share changed, the old share's tone stayed
in the recording at -12 dBFS for the rest of the recording (three runs). In
one of them the page's status line was read: it reported that the track had no
attached element. Detaching an audio
element in Egress's Chrome did not stop the sound of a track that had been
played. The cause was not investigated. The current page connects with
`autoSubscribe: false` and subscribes only to cameras, shares, microphones and
the audio of the main share; with that, the old share's tone is absent after
a switch (the table above and R6).

Other facts about the page (**observed**):

- The composite sound is what the page plays in Chrome (the Egress log shows
  `creating pulse sink` and `sdkSource: false`), so the page decides the mix.
- The recording continues until `StopEgress`. When the last participant left
  and nobody stopped the egress, the page received the room's end 20 s later
  (the room's departure timeout) and wrote `END_RECORDING`. The file has 20 s
  without sound at its end. The picture of those 20 s was not viewed.
- The tile column held 5 tiles at 1280x720 (observed). By the page's style
  rules a sixth tile would be cut (not observed). The layout of many tiles is
  not part of this test.

## R2. Separate audio files: PASS

The controller started one `StartTrackEgress` per audio track. Result
(**observed**), from `out/call-a/analysis.json`:

| File (track) | Media length | Expected tone | Level of that tone | Strongest other tone | End reason |
|---|---|---|---|---|---|
| alice microphone | 231.45 s | 440 Hz | -12.0 dBFS | -93.7 dBFS | StopEgress API |
| alice share audio | 231.35 s | 1000 Hz | -12.0 | -91.9 | StopEgress API |
| bob microphone, first connection | 153.17 s | 620 Hz | -12.0 | -87.5 | Source closed |
| bob share audio, first connection | 153.11 s | 1370 Hz | -12.0 | -94.4 | Source closed |
| bob microphone, second connection | 74.85 s | 620 Hz | -12.0 | -83.8 | StopEgress API |
| bob share audio, second connection | 74.87 s | 1370 Hz | -12.0 | -90.6 | StopEgress API |
| carol microphone | 173.43 s | 790 Hz | -12.1 | -83.5 | StopEgress API |
| carol share audio | 169.37 s | 1730 Hz | -12.0 | -96.5 | StopEgress API |

One file per track, each with only its own tone.

Container and codec (**observed**): Ogg container, Opus, 48 kHz, 2 channels,
117 to 130 kbit/s, 20 ms packets. File names end in `.ogg`; Egress adds the
extension.

Egress transcodes (**observed**). Evidence:

- The Ogg comment header of every file says `Encoded with GStreamer opusenc`.
- The test participant publishes every audio track in mono. All files are
  stereo, at 117 to 130 kbit/s.
- The Egress log for a track egress shows a decoding and mixing pipeline
  (`adding src TR_..._0 to audio`, `adding src audio_test_src to audio`).
- The unified `StartEgress` request with `preset: PASSTHROUGH` for one audio
  track was accepted and also produced a file encoded by `opusenc` (stereo,
  130 kbit/s). The unified request without a preset gave the same.

No request form tested here stores the original Opus packets. A person who
downloads an audio file gets a re-encoded copy at about 128 kbit/s, not the
stream that was sent. The track files of the restart tests decoded with ffmpeg
without an error message.

## R3. Reconnect and mute: PASS

### Reconnect (observed)

bob's process closed its connection and joined again 3 s later. Wall-clock
times in seconds, relative to the moment bob left. bob's times are from the
host's clock and Egress's times from the Docker VM's clock; the two differed
by 0.03 to 0.07 s:

| Time | Event |
|---|---|
| 0.000 | bob leaves. |
| +0.20 | `file_results.ended_at` of both of his track egresses. |
| +0.25 | `ended_at` of both egresses. Status `EGRESS_COMPLETE`, `details` = `End reason: Source closed`, `error` empty. |
| +0.54 | The controller sees the change in `ListEgress` (it polls once per second). |
| +3.09 | bob has joined again and published a new microphone track and a new share-audio track. Both have new track identifiers. |
| +3.57 | The controller sees the new tracks in `ListParticipants` and calls `StartTrackEgress` for each. |
| +3.82 and +3.86 | `file_results.started_at` of the two new files. |

- The old files end complete and playable (153 s each). Egress does not wait
  for the participant and does not continue with the new track.
- **What the controller must do:** start a new track egress for every new
  audio track. In this test it found new tracks by calling `ListParticipants`
  once per second. The new files miss the first 0.71 and 0.77 s of the new
  tracks (0.48 s until the poll saw them, then 0.25 to 0.29 s until Egress
  started the file).
- The composite recording needed no action: the page showed bob's new share
  as the main picture (it is the most recently started one) and played its
  tone (period 7 in the table under R6).

### Mute for 20 seconds (observed)

alice's microphone was muted at 195 s of the run and unmuted 20.0 s later
(exact to 0.1 s).

- The track egress stayed `EGRESS_ACTIVE`. The file did not end.
- The muted period is in the file as silence: the tone stops at file position
  182.800 s and returns at 202.808 s, 20.008 s later.
- The file has no timestamp gap: 11 573 packets, every packet 20 ms after the
  previous one. During the mute the packets are about 3 bytes each.
- The file's media length (231.45 s) equals the wall-clock time between
  `file_results.started_at` and `file_results.ended_at` (231.42 s).
- In the composite MP4 the 440 Hz tone is absent during the mute (-88.3 dBFS)
  and present afterwards (-12.0 dBFS).

**Not verified:** a mute much longer than 20 s, and a moderator's microphone
block (the server removes the permission to publish), which may unpublish the
track and so end the file.

### Timestamps that Egress reports (observed)

`ListEgress` returns for each egress (times in nanoseconds since the Unix
epoch, as strings in JSON):

| Field | Meaning seen in this test |
|---|---|
| `started_at` | The start request was accepted. 0.20 to 0.24 s before the media of a track file begins, 3.9 s before the media of the composite begins. |
| `file_results[0].started_at` | The beginning of the media in the file. Use this one. |
| `file_results[0].ended_at` | The end of the media in the file. |
| `file_results[0].duration` | Length in nanoseconds, equal to `ended_at - started_at`. The media length that ffprobe reports for a track file differs from it by at most 0.03 s. |
| `file_results[0].size`, `filename` | Bytes and the final path. Present only after the egress ended. |
| `ended_at` | The egress finished: 0.04 to 0.08 s after the file's end for track files and 0.47 s for the composite in `call-a` (28 s in the overloaded first attempt). |
| `updated_at` | The last status change. It does not advance while an egress records. |
| `status`, `error`, `details` | `EGRESS_STARTING`, `EGRESS_ACTIVE`, `EGRESS_ENDING`, `EGRESS_COMPLETE`; `details` holds the end reason. |

Egress also writes a manifest `<egress id>.json` beside each file with the
egress identifier, the room, the track identifier, `started_at`, `ended_at`
and the file name. It contains the egress times, not the file times.

### Alignment (observed)

Measured with the time markers of the three microphone tones in `call-a`.
"Error" is: the file's `file_results.started_at` plus the marker's position in
the file, minus the true time of the marker.

| Measurement | Markers | Median | Range |
|---|---|---|---|
| Track file against the wall clock | 60 | +0.034 s | +0.030 to +0.037 s |
| Composite audio against the wall clock | 47 | +1.011 s | +0.955 to +1.073 s (3 markers at -0.34 s fell on an audio drop, see below) |
| Composite audio minus its track file | 47 | +0.981 s | +0.925 to +1.036 s |
| Composite video against the wall clock | 23 | +0.949 s | +0.883 to +1.016 s |
| Composite video minus composite audio | 18 | -0.084 s | -0.114 to -0.002 s |

- Every track file, including bob's second segments, can be placed on one
  timeline from `file_results.started_at` with an error of 30 to 37 ms. This
  figure includes the network path, Egress's buffer, and the difference
  between the host's clock and the Docker VM's clock (measured at 0.03 to
  0.07 s with a method that is exact to about 0.1 s).
- The composite MP4 is about 1.0 s late: an event at wall-clock time T is in
  the MP4 at `T - file_results.started_at + 1.0 s`. The offset varied by
  0.12 s inside this recording. In the overloaded first attempt it was 0.80 to
  0.99 s, so it is not the same in every recording.
- Inside the MP4 the picture is 0.08 s ahead of the sound (median). The video
  marker is exact to one camera frame (67 ms).
- A transcript built from the track files and shown against the MP4 needs
  this offset of about 1 s, or its lines appear 1 s early.

Audio drops in the composite (**observed**): Egress logged `Can't record audio
fast enough` twice for the composite of `call-a`. In the MP4, between 109.4
and 111.7 s, the sound stopped several times for 0.02 to 0.3 s. In the track
files of the same call no marker is displaced (the 60 markers are within 7 ms
of each other). The host was loaded by other work (see
"Versions and environment"). The 1080p run of R5 had no such event.

## R4. State recovery: FAIL for an Egress restart, PASS for a LiveKit restart

In each run a composite recording and four track egresses had been recording
for about 32 s when the container was restarted.

### Egress container: `docker restart`, and `docker kill` then `docker start` (observed)

- `docker restart junto-spike-rec-egress` returned after 1.4 s. Egress logged
  `exit requested, finishing recording then shutting down` and `draining`, and
  the new process started 1.5 s later. Why Docker ended the old process after
  about 1 s was not investigated.
- **The running recordings are lost.** The output directory has no file for
  any of the five egresses, not a partial one. The container's start script
  deletes Egress's temporary directory (`rm -rf /home/egress/tmp/*`), and after
  the restart that directory held nothing from the old process. That Egress
  writes there and moves a file to the output path only when the egress ends
  is inferred from this and from the log line `file upload completed` at the
  end of every egress. Nothing is playable because nothing is left.
- **`ListEgress` keeps returning all five with status `EGRESS_ACTIVE`,**
  `ended_at` 0, empty `error`. This was still the case 35 minutes later, when
  the stack was removed, long after their rooms had closed. `ListEgress` with
  `active: true` returns them too.
- `StopEgress` for such an egress returns HTTP 408 with
  `{"code":"deadline_exceeded","msg":"request timed out"}` after about 3 s.
- The egress's participant (identity = the egress identifier, kind `EGRESS`)
  stayed in `ListParticipants` for 20 s after the restart and was gone at
  40 s. The room's `active_recording` was `true` at 20 s and `false` at 40 s.
- Egress does not resume anything after it starts again. New start requests
  work at once.
- `docker kill` (SIGKILL) followed by `docker start` gave the same result for
  the files, for `ListEgress` and for `StopEgress`.

### Egress container: stop with enough time (observed)

- `docker kill -s TERM junto-spike-rec-egress` (SIGTERM, no forced end): Egress
  kept recording. 75 s later the controller called `StopEgress` for the five
  egresses; all ended `EGRESS_COMPLETE` with complete, playable files, and
  then the Egress process and its container exited.
- `docker stop -t 60` and `docker restart -t 40` with one track egress
  running: the container kept running and the egress stayed active; after
  `StopEgress`, which the test sent 10 s later, the file was complete and the
  Docker command returned about 1 s after that.
- So Egress finishes its recordings after SIGTERM when it is given the time.
  The default of `docker restart` did not give it the time.

### LiveKit container: `docker restart` (observed)

- The restart took 1.6 s. Both participants (Rust SDK) reconnected by
  themselves 4.2 s after losing the connection. Their tracks got new
  identifiers.
- The four track egresses ended `EGRESS_COMPLETE` with `End reason: Source
  closed`. Their files are complete and playable (39 s each).
- The controller saw four new audio tracks and started four new track
  egresses; their files are complete and playable (69 to 70 s each).
- The composite egress stayed `EGRESS_ACTIVE`. The page in Chrome reconnected
  by itself. The MP4 is one playable file of 107 s. Its sound is missing from
  33 s to 43 s (10 s), then continues.
- After the restart the page chose a different main share than before
  (alice's instead of bob's): both shares were published again at almost the
  same time, and no main share had been named. **Not verified:** whether room
  metadata set before a LiveKit restart is still there afterwards.
- `ListEgress` returns every egress from before the restart with the correct
  status. Redis was not restarted. That LiveKit keeps the egress records in
  Redis was not verified separately.

**Not verified:** a restart of Redis, a network interruption between Egress
and LiveKit without a restart, and a restart of LiveKit that takes longer than
the SDK's reconnection attempts.

### API calls for reconciliation

1. `ListEgress` with `room_name` (or `active: true`): the status, the times and
   the file results of every egress. It is correct for egresses that ended
   normally and after a LiveKit restart. It is wrong after an Egress restart.
2. `ListParticipants` for the room: an egress that records has a participant
   whose identity is its egress identifier (kind `EGRESS`, hidden for
   clients). An egress that `ListEgress` reports as active, that started more
   than 40 s ago, and that has no such participant is dead. Observed for a
   track egress and for a composite egress while recording, and for a track
   egress after an Egress restart.
3. `StopEgress`: HTTP 200 for a live egress; HTTP 408 `request timed out` for a
   dead one.
4. `ListParticipants` again for the audio tracks that exist now, to start the
   missing track egresses, and `StartRoomCompositeEgress` when no live
   composite exists.

## R5. Limits: measured

### Simultaneous egress requests (observed)

- `limits`: with one 720p30 composite recording, 30 `StartTrackEgress`
  requests for one microphone track were sent one per second. All 30 were
  accepted and recorded; all 31 ended `EGRESS_COMPLETE`. The test stopped at
  30, so the limit is higher than 31. At that moment the Egress container used
  215 % of one CPU core and 1.14 GiB.
- `limits`: 4 simultaneous 720p30 composites were accepted and completed
  (422 % CPU, 1.84 GiB at that moment).
- `probe`: 5 simultaneous 720p30 composites were accepted and completed. The
  request for a sixth got no answer within the client's 20 s limit. At that
  moment the Egress container used 882 % CPU (of 800 % nominal) and 3.47 GiB.
- `probe2`: with the Egress container stopped, `StartTrackEgress` returned
  HTTP 503 `{"code":"unavailable","msg":"no response from servers"}` after
  22 s.
- Egress logs at start: `cpu available: 8.000000 max cost: 4.000000`. It
  accepts a request when it measures enough idle CPU, so the number depends on
  the load at that moment. **Not verified:** the configured cost per request
  type.
- Start calls returned in 0.04 to 0.6 s when no other composite was running or
  starting. A composite start call took 6 to 14 s to return when 1 to 4 other
  composites were running, and one track start call took 19.6 s while a
  composite was starting. The reason was not investigated.

### CPU and memory of Egress, composite plus four track egresses (observed, `docker stats`)

| Composite preset | Samples | CPU of the Egress container, mean | CPU, highest sample | Memory |
|---|---|---|---|---|
| `H264_720P_30` (`call-a`, 2 shares and 2 cameras) | 13 | 94 % of one core | 221 % | 0.62 to 0.67 GiB |
| `H264_1080P_30` (`stats-1080p`, 2 shares and 2 cameras) | 12 | 149 % | 287 % | 0.93 GiB |
| `H264_720P_30` plus 6 track egresses (`call-a`, 3 shares and 3 cameras) | 34 | 101 % | 115 % | 0.74 to 0.76 GiB |

Egress's own figures per egress (its `egress metrics` log line): a 720p30
composite 0.63 to 0.65 core on average and 2.3 to 2.5 at peak; a 1080p30
composite 1.17 on average and 2.41 at peak; a track egress 0.04 to 0.05 core
and about 82 MB. The LiveKit container used 8 to 11 % of one core on average
(highest sample 30 %) and 0.12 GiB with three participants.

These are figures for arm64 cores of the test machine with generated test
patterns, and with other work on the machine. They are not a figure for the
x86-64 data host.

### File sizes (observed)

| File | Size per minute | Bitrate |
|---|---|---|
| Composite MP4, `H264_720P_30`, 3 participants | 12.5 MB | H.264 Main 1280x720 at 30 fps, 1.5 Mbit/s; AAC-LC 44.1 kHz stereo, 115 kbit/s |
| Composite MP4, `H264_720P_30`, 2 participants | 10.8 to 11.0 MB | |
| Composite MP4, `H264_720P_30`, 1 participant | 4.9 to 6.1 MB | |
| Composite MP4, `H264_1080P_30`, 2 participants | 26.2 MB | H.264 1920x1080, 3.3 Mbit/s; AAC 129 kbit/s |
| Track file, Ogg/Opus | 0.96 to 0.98 MB (0.88 MB with 20 s of mute in 231 s) | 128 kbit/s |

The video size depends on the content. The API definition gives 3 Mbit/s for
the 720p30 preset and 4.5 Mbit/s for the 1080p30 preset.

## R6. Control by API: PASS

Start and stop (**observed**): `StartRoomCompositeEgress` with
`custom_base_url` and `StopEgress` worked in every run. The unified
`StartEgress` request with a `template` source and `custom_base_url` worked
too (`probe`), and `UpdateLayout` works on an egress started that way.
`StopEgress` returned in under 0.1 s, and the status became `EGRESS_COMPLETE`
1.1 s later.

Telling the page which share is the main one (**observed**, `call-a` and
`probe`). The page accepts a track identifier or a participant identity. Each
row was verified by the tones in the MP4 and by the frame (main picture and
the page's status line):

| Way | How it reaches the page | Level of the named share's tone | Other share tones | Status line |
|---|---|---|---|---|
| `layout` in the start request (`main:alice;debug`) | Query parameter `layout` of the page URL | 1000 Hz present from the first second (`probe`); without it the page would have chosen bob | 1370 Hz absent | not viewed |
| `UpdateLayout` (`main:alice;debug`) | LiveKit sets the metadata of the recorder's own participant to JSON with a `layout` key; the page reads it in `ParticipantMetadataChanged` | 1000 Hz at -12.1 dBFS | -83.4 and -90.0 dBFS | `main=alice by=update-layout` |
| Room metadata (`{"main_share":"bob"}`) | `RoomMetadataChanged` | 1370 Hz at -12.1 dBFS | -71.5 and -86.7 dBFS | `main=bob by=room-metadata` |
| Data message, topic `layout` (`{"main_share":"alice"}`) | `DataReceived` | 1000 Hz at -12.1 dBFS | -86.3 and -92.8 dBFS | `main=alice by=data-message` |
| Cleared (empty id) | as above | 1730 Hz at -12.1 dBFS (carol, the latest share) | -86.5 and -84.9 dBFS | `main=carol by=auto` |
| After bob's reconnect, no instruction | automatic | 1370 Hz at -12.0 dBFS (bob's new share is the latest) | -81.4 and -88.2 dBFS | `main=bob by=auto` |

Speed (**observed**): the named share's tone is audible in the MP4 1.1 to
1.2 s after the time of the API call when the MP4 is placed by
`file_results.started_at`. The MP4 itself is 1.0 s late (R3), so the page
switched 0.1 to 0.2 s after the call. For carol's new share the figure is
1.4 s, so about 0.4 s.

Differences between the ways:

- `UpdateLayout` needs the egress identifier and reaches only that recorder.
  The page received it as a change of its own participant's metadata.
- Room metadata is visible to every client in the room. **Not verified:**
  whether it survives a LiveKit restart.
- A data message is delivered once to the participants that are connected at
  that moment (by the API's definition). **Not verified:** what a page that
  reconnects later knows, and a data message sent only to the recorder's
  identity.
- The `layout` string in the start request sets the state before the first
  recorded frame.

## Other observations

1. **A composite recording under CPU starvation loses sound without an
   error (observed).** In the first attempt of `call-a` the host's load
   average rose to 88 because of unrelated work. The MP4's sound stopped at
   90 s and returned at 221 s; the audio packets in the file have gaps of up
   to 25 s. Egress logged `Can't record audio fast enough` and `Dropped N
   samples` repeatedly. The egress stayed `EGRESS_ACTIVE` and ended
   `EGRESS_COMPLETE` with an empty `error`. Its end took 28 s instead of 1 s.
   One track egress logged `buffer full, dropping sample` 14 times. The files
   of that attempt were deleted before the repeat run.
2. **Egress's debug log contains the recorder's room token (observed).** With
   `logging.level: debug` Egress writes the full page URL, including `token=`,
   to its log.
3. **Third-party STUN servers (observed).** The Egress log shows requests to
   public STUN servers of two third parties when an egress connects to the
   room. The stack's configuration names no STUN server, so this is a default.
   The design's rule of no connection to third parties needs a setting that
   turns it off. **Not verified:** which setting does that (`rtc.stun_servers`
   in the LiveKit configuration is the candidate).
4. **A recorder is a participant (observed).** `ListParticipants` returns one
   participant per egress, with kind `EGRESS`. The room's `num_participants`
   did not count it. The chat server's comparison of participants with the
   people allowed in a room must skip this kind.
5. **Local development network (observed).** With `--dev` and no `node_ip`,
   participants on the host connected to LiveKit in Docker through the ports
   published on the loopback address, and Egress in the same Compose network
   connected to LiveKit's container address.
6. **The Rust SDK on macOS (observed).** A binary linked without `-ObjC` stops
   at start. `../test-participant/build.rs` adds the flag.
7. **LiveKit API tokens (observed).** A `RoomService` call needs a token whose
   `room` grant names that room; a token with `roomAdmin` and an empty room
   name got HTTP 401.

## What the product's recording controller must do

Derived from the observations above.

1. **Name the main share explicitly.** Put `main:<id>` in the `layout` of the
   start request, and call `UpdateLayout` whenever the main share changes.
   Send the current value again after the recorder reconnects or a LiveKit
   restart. The page's own order of shares is a fallback.
2. **Serve a layout page that subscribes only to what it plays.** No
   subscription to the audio of a share that is not the main one. The recorded
   sound is what the page plays.
3. **Start one track egress per audio track, and one for every track that
   appears later.** A reconnect and a LiveKit restart each gave new track
   identifiers; a share that stops and starts again was not tested. Find new
   tracks from a periodic `ListParticipants` (with a 1 s poll the new file
   missed 0.7 to 0.8 s) and from the `track_published` webhook (webhooks were
   not tested). Store per segment: recording,
   participant, source, track identifier, egress identifier, and
   `file_results.started_at`, `ended_at`, `duration`, `filename`.
4. **Build the timeline from `file_results.started_at` of the track files.**
   They were within 37 ms of the wall clock. Do not use the egress
   `started_at`. Treat the composite MP4 as about 1.0 s late against its own
   `file_results.started_at` when transcript lines are shown against it; the
   offset was between 0.80 and 1.07 s in the two recordings measured.
5. **Stop every egress with `StopEgress` when the recording ends.** Without it
   a track egress ends only when its track ends, and the composite ends 20 s
   after the room is empty.
6. **Do not trust `EGRESS_ACTIVE` from `ListEgress` alone.** At an interval and
   after its own restart, the controller compares `ListEgress` for the room
   with `ListParticipants`: an active egress older than 40 s without a
   participant of the same identity is lost. Mark its segment failed with the
   time range that was saved (none after an Egress crash), and start a new
   egress if the call still runs. `StopEgress` answering 408 confirms a lost
   egress.
7. **Treat an Egress restart as the loss of every recording in progress.**
   Nothing partial is kept. Stop Egress only with a long stop timeout, after
   the controller has ended the recordings, and never with a plain
   `docker restart`. `docker stop -t` and `docker restart -t` were tested;
   `stop_grace_period` is the Compose setting for the same timeout and was not
   tested. The
   upgrade rule of the design (no migration while a recording runs) should
   cover the Egress container.
8. **Use a timeout above 22 s for start calls, and handle refusal.** With no
   free Egress instance the answer is HTTP 503 `no response from servers`
   after 22 s. Show that as "recorder not available".
9. **Check the result, not only the status.** A composite can end
   `EGRESS_COMPLETE` with missing sound when the host is short of CPU. Reserve
   CPU for Egress (on the test machine one 720p30 composite used 0.65 core on
   average and 2.5 at peak, one 1080p30 composite 1.2 and 2.4), limit
   simultaneous recordings, and compare
   the MP4's length and audio with the track files after the recording.
10. **Keep Egress's log level above debug in production**, or keep the log
    private: the debug log contains the recorder's token.
11. **Skip participants of kind `EGRESS`** in the admission check that removes
    people who are not allowed in a room.

## Not verified

- Egress on the x86-64 Linux data host, and its CPU use there.
- Recording of real screen content and real speech; every source was a
  generated pattern or tone.
- Webhooks. This test polled `ListEgress` and `ListParticipants`.
- A recording longer than 4 minutes: drift between files over an hour, file
  sizes of long recordings.
- The storage guard: what Egress does when the disk is full.
- Redis restart, network interruption between the hosts, and Egress behind
  the WireGuard link of the first deployment.
- H.264 or VP9 sources, simulcast, and more than three participants.
- A run on an idle machine. The audio drops in the composite may come from
  the load of other work on the test machine.

## What remains after the test

Deleted: the stack with its network (`./scripts/stack.sh down -v`; it had no
volumes), the layout page image, the `livekit/livekit-server` and
`livekit/egress` images, the build directory `../test-participant/target/`,
every recorded MP4 and Ogg file, and the temporary logs outside the
repository.

Left in place:

- `spikes/recorder/`: `compose.yaml`, `livekit.yaml`, `layout/`, `scripts/`,
  `requests/`, `.gitignore`, this file.
- `spikes/recorder/.env.local`: the generated API key and secret of the local
  stack. Not tracked. The stack needs it to start again; `stack.sh` creates a
  new one when it is missing.
- `spikes/recorder/out/`: controller logs, participant logs, `ListEgress`
  answers, `analysis.json`, selected lines of the Egress log, and the
  extracted frames (PNG). 3.4 MB, not tracked. They are the evidence for this
  file.
- `spikes/test-participant/`: source, `build.rs`, `Cargo.toml`, `Cargo.lock`,
  `README.md`.
- The Docker images `redis:7-alpine` and `nginx:alpine`. They are small base
  images, and it could not be determined whether they were on the machine
  before this test.
- The `lk` tool (Homebrew formula `livekit-cli`), installed for this test.
- Cargo's registry cache with the downloaded crate sources, outside the
  repository. The prebuilt WebRTC library was inside `target/` and is deleted
  with it; the next build of the test participant downloads it again.
