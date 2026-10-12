# test-participant

A synthetic LiveKit participant for media tests. It joins a room and publishes
generated audio and video, or joins as a subscriber and reports what arrives.
It needs no microphone, camera or screen. It does not use the SDK's platform
audio mode, so it opens no audio device and plays no sound.

It was written for the recorder spike (`../recorder`) and is meant to be reused
for the capacity tests. The second tier of the call tests of the server and the
recording test of the deployment (`deploy/tests/recording.sh`) run it. It is a
test tool, not product code.

## Build

```sh
../../tools/serial.sh cargo build --release
```

The first build downloads a prebuilt WebRTC library (several hundred MB into
`target/`). The binary is `target/release/test-participant`.

`build.rs` passes `-ObjC` to the linker on macOS. Without it the binary stops
at start with `+[NSString stringForStdString:]: unrecognized selector`.

Unit tests of the tone detector: `../../tools/serial.sh cargo test --release`.

Versions used: `livekit` 0.9.4 and `livekit-api` 0.8.2 (both Apache-2.0),
`libwebrtc` 0.3.51.

## Connection

Every command takes the same connection options. Each also reads an
environment variable.

| Option | Environment | Meaning |
|---|---|---|
| `--url` | `LIVEKIT_URL` | Server URL. Default `ws://localhost:7880`. |
| `--token` | `LIVEKIT_TOKEN` | A ready join token. |
| `--api-key` | `LIVEKIT_API_KEY` | Used to create a token when `--token` is absent. |
| `--api-secret` | `LIVEKIT_API_SECRET` | Used to create a token when `--token` is absent. |
| `--room` | | Room name. Required without `--token`. |
| `--identity` | | Participant identity. Required without `--token`. |

A token that the tool creates is valid for one hour and grants joining that
one room.

## publish

```sh
test-participant publish --room demo --identity alice \
  --mic-hz 440 --camera \
  --screen --screen-audio-hz 1000 --screen-color 2048c8 \
  --marker-period-ms 10000 \
  --mute-after 60 --unmute-after 80 --reconnect-after 120 --leave-after 180
```

Tracks:

| Track | Source | Content | Options |
|---|---|---|---|
| Microphone | `microphone` | Sine tone, mono, 48 kHz | `--mic-hz` (default 440), `--no-mic` |
| Camera | `camera` | Colour bars, a moving white line, the label `CAM <label>` and a frame counter | `--camera`, `--camera-size` (default 640x360), `--camera-fps` (default 15) |
| Screen share | `screen_share` | One background colour with a grid, a moving square, the label `SCREEN <label>` and a frame counter | `--screen`, `--screen-size` (default 1280x720), `--screen-fps` (default 5), `--screen-color RRGGBB` |
| Share audio | `screen_share_audio` | Sine tone, sent at 128 kbit/s without DTX and without redundancy | `--screen-audio-hz` (needs `--screen`) |

Sound from a file: `--mic-file <file.wav>` and `--screen-audio-file <file.wav>`
(needs `--screen`) publish a WAV file with 16-bit PCM and one channel, at any
sample rate, in place of the tone. With `--play-on-stdin` a track carries its
tone (the share audio: silence, without `--screen-audio-hz`) until the line
`play` arrives on standard input; then every file starts, and after a file its
track is silent. Without that option a file starts with its track. The output
has a `file_started` line with the length of the file and a `file_ended` line.

Other options: `--level` (tone peak, 0 to 1, default 0.25, which is -12 dBFS),
`--label` (default: the identity), `--simulcast` (off by default). Video is
VP8. Echo cancellation, noise suppression and gain control are off for every
audio track, because they would change a steady tone.

Timed actions. Each time is in seconds from the start of the program:

| Option | Action |
|---|---|
| `--screen-after N` | Start the screen share (video and audio) at N instead of at the start. |
| `--screen-stop-after N` | Stop the screen share. |
| `--mute-after N` | Mute the microphone track. |
| `--unmute-after N` | Unmute it. |
| `--reconnect-after N` | Leave the room, wait `--reconnect-gap` seconds (default 2), and join again. This is a new connection: the participant and every track get new identifiers. A share that was running is published again; a muted microphone is muted again. |
| `--leave-after N` | Leave the room and exit. Without it the program runs until Ctrl-C. |

Time marker (`--marker-period-ms P`, `--marker-gap-ms G`, default gap 300): at
every Unix-time multiple of `P`, for `G` milliseconds, the microphone tone is
silent and the lower third of both video patterns is white. The marker follows
the wall clock, so every participant on one machine marks the same moment, and
a recording can be compared with the clock. Share audio has no marker.

Output: one JSON object per line on standard output.

```json
{"event":"joined","identity":"alice","participant_sid":"PA_...","room":"demo","unix_ms":1791519455274}
{"event":"published","name":"microphone","source":"microphone","tone_hz":440.0,"track_sid":"TR_...","unix_ms":1791519455277}
{"action":"Mute","event":"action","t":195.1,"unix_ms":1791519649927}
{"event":"left","identity":"alice","unix_ms":1791519716931}
```

Other events: `reconnecting` and `reconnected` (the SDK's own reconnection
after a network or server failure, which also gives new track identifiers) and
`disconnected` (the server closed the connection; the program exits).

Messages of the SDK go to standard error when `RUST_LOG` is set, for example
`RUST_LOG=info`.

## subscribe

```sh
test-participant subscribe --room demo --identity probe --duration 30 \
  --expect alice/microphone=440 --expect alice/screen_share_audio=1000
```

It subscribes to every track. Every `--report-every` seconds (default 5) it
prints a `report` line, and at the end a `summary` line, each with one entry
per track:

| Field | Meaning |
|---|---|
| `participant`, `track_sid`, `kind`, `source`, `mime`, `muted` | The track. |
| `packets_received`, `packets_lost`, `bytes_received` | From the WebRTC statistics of the receiving connection. |
| `audio_frames` | Decoded 10 ms audio frames received. |
| `video_frames`, `width`, `height` | Decoded video frames received and the last frame size. |
| `level_dbfs` | RMS level of the last second of audio. |
| `tones_now` | Sine tones found in the last 200 ms of each second: frequency in Hz (10 Hz steps, 50 to 4000 Hz) and level in dBFS. |
| `tone_seconds` | For each tone frequency, the number of seconds in which it was found. |

`--expect identity/source=hz` (repeatable; `source` is `microphone` or
`screen_share_audio`) states the tone a track must carry. An expectation is
met when the frequency found in the most seconds is within 10 Hz. The exit
status is 1 when a track is missing or carries another tone, and the `summary`
line lists the reason under `failures`.

## Source files

| File | Content |
|---|---|
| `src/cli.rs` | Options. |
| `src/session.rs` | Token creation and connection. |
| `src/publish.rs` | Publishing and the timed actions. |
| `src/subscribe.rs` | Subscriber mode and expectations. |
| `src/tone.rs` | Tone generator, WAV reader, time marker, tone detector (Goertzel sweep). |
| `src/pattern.rs` | Video patterns and the 5x7 font. |
| `src/events.rs` | JSON output lines. |

## Limits

- The video patterns are drawn and encoded on the CPU. The CPU use of the tool
  was not measured.
- The tone detector reports a frequency to 10 Hz. Use tone frequencies that are
  multiples of 10 Hz and at least 50 Hz apart.
- The marker is exact to one audio sample at the sender. On video it is exact
  to one frame of the pattern (67 ms at 15 fps).
