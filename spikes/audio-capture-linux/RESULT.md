# Spike: audio capture on Linux with PipeWire

Date of the test: 2026-10-09.

## Question

Can a Rust program on a Linux desktop with PipeWire capture (a) the sound of one
chosen application only and (b) all system sound without its own playback, and
deliver PCM samples in real time?

## Result

Yes. Every gate passed.

| Gate | Result | Evidence in one line |
|---|---|---|
| L1 app-only capture | PASS (observed) | Wanted tone at -14.0 dBFS, the tones of the two other processes at -128 dBFS or lower |
| L2 all system sound without own sound | PASS (observed) | Both tones at -14.0 dBFS, own tone at -160 dBFS (analysis floor) while the sink mix had it at -14.0 dBFS; a stream that started 5 s after the capture was linked 13 ms after it appeared |
| L3 stream restart | PASS (observed) | Three new nodes of one process, two new processes matched by binary name, two new processes matched by application name: all captured |
| L4 format and latency | PASS (observed) | F32LE, 48 000 Hz, 2 channels; quantum 256 to 2048 frames (5.3 to 42.7 ms); samples arrive in the same graph cycle in which the application wrote them |
| L5 system requirements | PASS (observed, parts reasoned) | PipeWire 1.6.8, WirePlumber 0.5.15; native, PulseAudio and ALSA clients captured; Wayland not verified |
| L6 sandboxes | PASS for a simulated Flatpak sandbox (observed); real Flatpak and AppImage builds not verified | A client that PipeWire classified as `flatpak` listed the graph, created the links and captured correctly |

## Test machine

A desktop PC reached over SSH. Observed:

- openSUSE Tumbleweed, X11 session of a normal user.
- `pipewire --version`: compiled and linked with libpipewire 1.6.8.
- Packages: `pipewire-1.6.8`, `wireplumber-0.5.15`, `pipewire-pulseaudio-1.6.8`,
  `pipewire-alsa-1.6.8`. No PipeWire development package on the host.
- Tools on the host: `pw-cli`, `pw-dump`, `pw-record`, `pw-play`, `pw-cat`,
  `pw-link`, `pw-loopback`, `pw-top`, `pw-metadata`, `wpctl`, `pactl`, `paplay`,
  `aplay`, `bwrap`. Not on the host: `ffplay`, `ffmpeg`, `sox`.
- Clock settings (`pw-metadata -n settings`): `clock.rate` 48000,
  `clock.quantum` 1024, `clock.min-quantum` 32, `clock.max-quantum` 2048.

## Method

The program is `junto-audio-spike` (this directory, Rust, crate `pipewire`
0.10.1). It does the capture itself through libpipewire. It does not call
`pw-link` or `pw-record`.

1. It connects to PipeWire and creates one capture stream (`pw_stream`,
   direction input, format F32LE, 48 000 Hz, 2 channels FL and FR). The stream
   is connected without the `AUTOCONNECT` flag and with `node.autoconnect=false`,
   so the session manager does not link it to anything.
2. It listens to the registry. It records every node with
   `media.class = Stream/Output/Audio` (a playback stream of an application),
   every port, and every client. It binds each `Client` object and reads its
   properties, because the process properties are on the client and not on the
   node (observed: the node has `client.id`; `application.process.id` is only on
   the client).
3. For each playback stream node it decides whether the node is wanted:
   - app-only mode: the client of the node matches the chosen application;
   - all-system mode: every playback stream node, except the nodes of the
     program itself.
4. For each wanted node it creates one link per channel from the output port
   of the node to the input port of the capture stream with the same channel
   (`audio.channel` FL to FL, FR to FR; a `MONO` port goes to both). The links
   are created with `link-factory` (`core.create_object`) and
   `object.linger=false`, so PipeWire destroys them when the program exits.
5. It repeats step 3 and 4 on every registry event, so streams that appear
   later are linked, and the links of a removed node are dropped.
6. The capture stream receives the mix of all linked nodes. PipeWire mixes the
   links on the input ports.
7. The capture callback runs only while at least one linked node runs. The
   program fills the time without callbacks with silence, from the monotonic
   time of each callback, so the output has a continuous time line.

The application's playback to its own output device is not changed. The
capture is an extra link from the same output port.

### How an application is identified

Observed properties of the client that owns a playback stream node:

| Client type | `application.process.id` | `pipewire.sec.pid` | `application.process.binary` | `application.name` | `client.api` |
|---|---|---|---|---|---|
| native (`pw-play`) | process id of the player | same process id | `pw-cat` | `pw-cat` | not set |
| PulseAudio (`paplay`) | process id of the player | process id of `pipewire-pulse` | `pacat` | `paplay` | `pipewire-pulse` |
| ALSA (`aplay -D pipewire`) | process id of the player | same process id | `aplay` | `PipeWire ALSA [aplay]` | `alsa` |
| sandboxed, native | `2` (id inside the sandbox) | process id on the host | `pw-cat` | `pw-cat` | not set |
| sandboxed, PulseAudio | `2` (id inside the sandbox) | process id of `pipewire-pulse` | `pacat` | `paplay` | `pipewire-pulse` |

Sandboxed clients also had `pipewire.access = flatpak` and
`pipewire.access.portal.app_id = <application id>`.

Consequences, all observed:

- A node id is not an identity. PipeWire reused node id 94 for the second
  stream of the same process; `object.serial` was different (1601, 1633, 1656).
- The process id (`application.process.id`) identifies an application across
  new nodes while the process lives. It is the id that the client library
  reports, inside the process-id namespace of the application. Two sandboxed
  applications both reported `2`.
- `application.process.binary`, `application.name` and
  `pipewire.access.portal.app_id` identify an application across process
  restarts.
- `pipewire.sec.pid` is read by the PipeWire daemon from the socket. For
  PulseAudio clients it is the process id of `pipewire-pulse`, not of the
  application.

### How the program excludes its own sound

A node is the program's own when its `client.id` is the client of the capture
stream, or when its client has the same `pipewire.sec.pid` as the client of the
capture stream. The first version compared `application.process.id` with the
program's own process id. That rule was wrong inside a sandbox: the program had
process id 2 and it excluded another sandboxed application that also reported
2 (observed; the 1000 Hz tone was missing). The rule above fixed it (observed).

### PipeWire objects and properties used

| Object | Used for | Properties or fields |
|---|---|---|
| `Node` (registry global) | find playback streams | `media.class`, `client.id`, `node.name`, `application.name`, `object.serial`; `node.link-group` from the node info (optional rule) |
| `Client` (bound, info event) | identify the application | `application.process.id`, `pipewire.sec.pid`, `application.process.binary`, `application.name`, `client.api`, `pipewire.access.portal.app_id` |
| `Port` (registry global) | find the ports to link | `node.id`, `port.direction`, `audio.channel`, `port.monitor` |
| `Link` (created) | route the audio | `link.output.node`, `link.output.port`, `link.input.node`, `link.input.port`, `object.linger=false`; factory `link-factory` |
| capture stream | receive PCM | `media.type=Audio`, `media.category=Capture`, `media.role=Production`, `node.autoconnect=false`, optional `node.latency` |
| stream time (`pw_stream_get_time_n`) | delay field | `delay` |

## Test setup

- A null sink `junto-spike-sink` (`pw-cli create-node adapter` with
  `factory.name=support.null-audio-sink`, `media.class=Audio/Sink`,
  `object.linger=true`, `priority.session=0`). Every test sound was played into
  this sink only.
- Tones: 440 Hz (A), 1000 Hz (B), 1700 Hz (D), 3300 Hz (E) as 16-bit WAV files,
  amplitude 0.2 (-14.0 dBFS). 2500 Hz (C) is the tone that the capture program
  plays itself.
- Players, each a separate process: `pw-play` (native), `paplay` (PulseAudio
  layer), `aplay -D pipewire` (ALSA plugin), and the `play` command of the
  spike program (native).
- Safety: every player had the test sink as target and
  `node.dont-fallback=true`. A check with a silent file showed that a stream
  with a missing target fails ("defined target not found") and is not moved to
  the default sink. A watcher polled the graph every 0.25 s during every test.
  The hardware sink stayed `suspended` in every poll and no link into it was
  seen (`out/logs/real-sinks-*.log`). In the loopback test the summary line
  counts 49 polls with a sink that is not suspended: that sink is the test's
  own loopback sink (node 76). The same log shows the hardware sink (node 60)
  as `suspended` in every line.
- Measurement: `junto-audio-spike analyze` computes, for each frequency, the
  amplitude of that frequency in the captured PCM (single-frequency Fourier
  sum with a Hann window), in dBFS. The floor of the output is -160 dBFS.
  Values between -120 and -135 dBFS appear for some frequencies; they are
  distortion products of the 16-bit tone files and are more than 100 dB below
  the tones.

## Gates

All numbers below are from the last complete run with the final program
(`out/logs/final-run.log`).

### L1 app-only capture: PASS (observed)

Three processes played at the same time into the test sink. The capture ran
three times, each time with `--mode app --pid <process id>`. Amplitude in the
range 1 s to 5 s:

| Captured process | 440 Hz | 1000 Hz | 1700 Hz |
|---|---|---|---|
| A, `pw-play`, native | -14.0 | -128.3 | -160.0 |
| B, `paplay`, PulseAudio layer | -160.0 | -14.0 | -160.0 |
| D, `aplay`, ALSA plugin | -160.0 | -160.0 | -14.0 |

### L2 all system sound without own sound: PASS (observed)

Method: link every `Stream/Output/Audio` node except the program's own nodes
to the capture stream, and do so again on every registry event.

A (440 Hz) and B (1000 Hz) played from the start. The program played C
(2500 Hz) into the same sink for the whole run. E (3300 Hz, `aplay`) started
5 s after the capture began. B was stopped at 10 s.

| Range | 440 Hz | 1000 Hz | 2500 Hz (own) | 3300 Hz (late) |
|---|---|---|---|---|
| 1 to 4.5 s | -14.0 | -14.0 | -160.0 | -160.0 |
| 6.5 to 9.5 s | -14.0 | -14.0 | -160.0 | -14.0 |
| 11.5 to 14.5 s | -14.0 | -128.3 | -160.0 | -14.0 |

- The own tone was real: `pw-record` recorded the monitor of the test sink in
  the same run and measured 2500 Hz at -14.0 dBFS.
- Control run with `--include-self`: 2500 Hz at -14.0 dBFS in all three ranges.
  This shows that the measurement detects the tone when the node is linked.
- Late stream: the node of E appeared at t = 5.083 s and its links were
  requested at t = 5.096 s. The 0.5 s window that starts at 5.0 s has 3300 Hz at
  -14.2 dBFS.
- Removed stream: the node of B was removed at t = 10.077 s; 1000 Hz is absent
  after it.

### L3 dynamic behaviour: PASS (observed)

A distractor (B, 1000 Hz, `paplay`) played during all three tests. It measured
-160.0 dBFS in every window that lies fully inside a tone period.

| Case | Selector | What happened | Tone in each period |
|---|---|---|---|
| a | `--pid` | One process created three streams one after the other (3 s each, 1.5 s apart). Node ids 94, 94, 92; `object.serial` 1601, 1633, 1656. | 440 Hz at -14.0 in all three |
| b | `--binary aplay` | `aplay` was started, stopped, and started again (two process ids). The capture started before the first one. | 1700 Hz at -14.0 in both |
| c | `--app-name` | The spike player ran twice as two processes with the same `application.name`. | 440 Hz at -14.0 in both |

Between the periods the capture had no linked node and received no callbacks.
The program filled these times with silence (for example 1.529 s and 1.519 s in
case a).

Windows that contain the start or the end of a tone show other frequencies
between -78 and -113 dBFS, lower for higher frequencies. This is the spectrum of
the abrupt start or end of the 440 Hz tone, not the distractor: the distractor
would measure -14 dBFS at 1000 Hz only.

Identity across node changes: see "How an application is identified".

### L4 format and latency: PASS (observed)

- Format delivered to the callback: `F32LE`, 48 000 Hz, 2 channels, positions
  FL and FR (interleaved). The program asked for this format; the graph rate was
  also 48 000 Hz.
- Quantum (frames per callback, from the callback data and from `pw-top`):

| Situation | Quantum | Interval between callbacks |
|---|---|---|
| Only the spike player and the capture, no `node.latency` | 1024 | 21.33 ms (min 21.23, max 21.71) |
| `pw-play`, `paplay` or `aplay` also playing (they ask for 4800, 8192 and 6000 frames) | 2048 (the configured maximum) | 42.67 ms |
| Capture stream with `node.latency=256/48000` | 256 | 5.33 ms (min 5.18, max 5.77) |
| The same while `paplay` also plays | 256 | 5.33 ms (min 1.36, max 9.33) |

  The quantum changed from 1024 to 2048 during one run when `paplay` started.
  The block size is therefore not constant.
- Delay: the player logged the monotonic time of the callback in which it
  wrote the first sample of the tone, and the capture logged the monotonic time
  of the callback in which it received that sample. Difference: 0.041, 0.064,
  0.034 and 0.028 ms in the four cases, at the same frame position in the block
  (768 of 1024 written, 769 detected: the first sample of a sine is 0). The
  capture adds no graph cycle. The samples of one block are therefore between
  0 and one quantum old when the callback runs: at most 5.3 ms with quantum 256
  and at most 42.7 ms with quantum 2048.
- `pw_time.delay` of the capture stream was 0 in these runs and 1024 in the L2
  runs. Not investigated.

### L5 what the method requires: PASS (observed, parts reasoned)

- PipeWire version on the machine: 1.6.8 (observed). The binary was built
  against the libpipewire 1.4.2 headers of Debian 13 in a container and ran with
  the host library 1.6.8 (observed).
- WirePlumber: version 0.5.15 was running (observed). The capture links were
  created by the program's own client and not by WirePlumber (observed: the
  links into the capture node carry the client id of the program and
  `object.linger=false`; the links into the sink carry the client id of
  WirePlumber). `link-factory` is a module of the PipeWire daemon. The capture
  without any session manager was not tested, because stopping WirePlumber was
  not allowed. A session manager is still needed for two things: applications
  get their link to an output device from it, and it grants the permissions of
  sandboxed clients.
- PulseAudio compatibility layer: works (observed, `paplay` in L1, L2, L3).
- ALSA clients through the PipeWire ALSA plugin: works (observed, `aplay -D
  pipewire` in L1, L2, L3). An ALSA program that opens a hardware device
  directly does not create a node in the graph and cannot be captured
  (reasoned, not verified).
- JACK clients were not tested. Their nodes do not have the media class
  `Stream/Output/Audio`, so the rule of this spike would not select them
  (reasoned, not verified).
- X11 against Wayland: the test session was X11. The method uses only the
  PipeWire socket of the user and no display server interface, so no difference
  is expected for the capture (reasoned, not verified on Wayland). One
  difference exists outside the capture: on Wayland an application cannot read
  the process id of another application's window, so "the application of the
  shared window" cannot be derived from the window. The person has to choose
  the application from the list of applications that play sound (reasoned, not
  verified).

### L6 sandboxes: PASS for a simulated Flatpak sandbox (observed); real builds not verified

- Test: the program ran inside a `bwrap` sandbox with its own process-id
  namespace and a `/.flatpak-info` file. PipeWire classified this client as
  `pipewire.access = flatpak` with `pipewire.sec.flatpak = true` (observed). No
  Flatpak service was started and no Flatpak application was run.
- In that sandbox, in all-system mode, the program captured a native player
  outside the sandbox (440 Hz at -14.0 dBFS) and a PulseAudio player inside a
  second sandbox (1000 Hz at -14.0 dBFS), and excluded its own tone (2500 Hz at
  -160.0 dBFS). It listed the registry, bound the `Client` objects, read their
  properties and created the links without an error (observed).
- A capture outside the sandbox selected the sandboxed player with
  `--portal-app-id` (`pipewire.access.portal.app_id`): 1000 Hz at -14.0 dBFS,
  440 Hz at -160.0 dBFS (observed).
- Reason it works: WirePlumber 0.5.15 gives a client with `flatpak` access the
  permissions read and execute on all objects
  (`/usr/share/wireplumber/scripts/client/find-flatpak-access.lua` on the test
  machine sets `Perm.RX`; a client with `media.category = Manager` gets all).
  A different session manager or a stricter configuration can refuse this
  (not verified).
- A real Flatpak build needs access to the PipeWire socket
  (`--filesystem=xdg-run/pipewire-0`). The PulseAudio socket alone does not give
  it (reasoned, not verified with a real Flatpak build).
- AppImage: an AppImage is not sandboxed, so it behaves as the native program
  did in L1 to L4 (reasoned, not verified). The program must load the
  libpipewire of the host and must not bundle its own copy, because libpipewire
  loads its plugins from the host (reasoned, not verified).

## Other observations

- Start of a new stream: for a stream that started while the capture already
  ran, the first sample reached the capture in the same graph cycle in 7 of 8
  starts (difference 0.07 to 0.27 ms). In 1 of 8 starts the first block of 2048
  frames (42.7 ms) was not captured: the application's node started before the
  link of the capture existed.
- Forwarding nodes: a virtual sink made with `pw-loopback` that forwards to
  another sink has a playback side with `media.class = Stream/Output/Audio`. In
  all-system mode the program linked both the application and that playback
  side, and the tone measured -8.0 dBFS instead of -14.0 dBFS (captured twice).
  With the option `--skip-link-groups` the program ignores nodes that have the
  property `node.link-group`, and the tone measured -14.0 dBFS (observed).
- WirePlumber wrote two entries for the two test sinks into its state file
  `stream-properties`. See "Proof that the audio graph was restored".

## Commands

`<alias>` is the SSH name of the test machine. All files on
the test machine were under `/srv/junto-spike/`.

```sh
# copy the source and the scripts
ssh <alias> 'mkdir -p /srv/junto-spike/scripts /srv/junto-spike/src /srv/junto-spike/ctx'
scp -r Cargo.toml Cargo.lock src <alias>:/srv/junto-spike/src/
scp scripts/* <alias>:/srv/junto-spike/scripts/
scp Dockerfile.build <alias>:/srv/junto-spike/ctx/Dockerfile

# on the test machine, as root
bash /srv/junto-spike/scripts/snapshot.sh before
nice docker build -t junto-spike-build:1 /srv/junto-spike/ctx
bash /srv/junto-spike/scripts/cargo.sh build --release     # container junto-spike-cargo, --cpus 6, nice
bash /srv/junto-spike/scripts/sink.sh create
bash /srv/junto-spike/scripts/route_check.sh               # silence only: where each player is linked
bash /srv/junto-spike/scripts/prepare_run.sh               # binary and tone files into run/
bash /srv/junto-spike/scripts/run_tests.sh l1 l2 l2-control l3a l3b l4
bash /srv/junto-spike/scripts/run_sandbox.sh
bash /srv/junto-spike/scripts/run_loopback.sh
bash /srv/junto-spike/scripts/sink.sh destroy
bash /srv/junto-spike/scripts/snapshot.sh after
```

The scripts run every PipeWire command as the user of the desktop session
(`runuser -u <user> -- env XDG_RUNTIME_DIR=/run/user/<uid> ...`); `env.sh` finds
the user with `loginctl`.

The program's commands, as the test scripts use them:

```sh
junto-audio-spike gen-wav --freq 440 --seconds 40 --amp 0.2 --out tone-a.wav
junto-audio-spike capture --mode app --pid <pid> --seconds 6 --out a.f32
junto-audio-spike capture --mode app --binary aplay --seconds 13 --out b.f32
junto-audio-spike capture --mode app --app-name <name> --seconds 13 --out c.f32
junto-audio-spike capture --mode app --portal-app-id <application id> --seconds 5 --out d.f32
junto-audio-spike capture --mode system --self-tone 2500 --self-target junto-spike-sink --seconds 15 --out s.f32
junto-audio-spike capture --mode system --include-self ...        # control
junto-audio-spike capture --mode system --skip-link-groups ...
junto-audio-spike capture ... --latency 256/48000
junto-audio-spike play --freq 440 --cycles 3 --seconds 3 --gap-seconds 1.5 --target junto-spike-sink --app-name <name>
junto-audio-spike analyze --in a.f32 --freqs 440,1000,1700,2500,3300 --ranges 1-5
junto-audio-spike analyze --in a.f32 --freqs 440,1000 --window 0.5
```

The players:

```sh
pw-play --target junto-spike-sink -P '{ node.dont-fallback=true node.dont-reconnect=true state.restore-props=false state.restore-target=false }' tone-a.wav
paplay --device=junto-spike-sink --property=node.dont-fallback=true --property=node.dont-reconnect=true tone-b.wav
PIPEWIRE_NODE=junto-spike-sink PIPEWIRE_PROPS='{ node.dont-fallback=true node.dont-reconnect=true }' aplay -q -D pipewire tone-d.wav
pw-record --target junto-spike-sink -P '{ stream.capture.sink=true }' --raw --format f32 --rate 48000 --channels 2 monitor.f32
```

Observed output is in `out/logs/` (not tracked): one capture log, one analysis
log and one watcher log per test, and `final-run.log` for the last complete run.

## Proof that the audio graph was restored

`scripts/snapshot.sh` saved, before the first change and after the cleanup:
`pw-dump`, a reduced list of all objects made from it
(`scripts/graph_summary.py`: clients, nodes with state, ports, links, modules,
devices, factories, metadata), `pw-cli ls` of nodes, links and modules,
`pw-link -l`, the `pactl` module, sink and sink-input lists, the default sink
with its volume and mute state, both metadata tables, the SHA-256 of
WirePlumber's three state files, and the start time of the three daemons.

Final comparison (`out/state-comparison.txt`): all 15 files identical.

- Graph objects before and after: 10 clients, 1 core, 6 devices, 13 factories,
  5 metadata, 13 modules, 8 nodes, 13 ports, 0 links. No object contains the
  text `junto`. All nodes are `suspended`, as before.
- Default sink, its volume (49 %) and its mute state: unchanged.
- `pipewire`, `pipewire-pulse` and `wireplumber`: same process ids and start
  times, so no daemon was restarted.
- The hardware sink was `suspended` in every poll of every test.

The first comparison after the tests showed two differences. Both were
corrected, and the comparison above is the one after the corrections.

1. WirePlumber had added two lines to its state file
   `~/.local/state/wireplumber/stream-properties`: the stored volume of
   `junto-spike-sink` and of the loopback test sink. The two lines were removed
   from the file. The SHA-256 of the file is again the SHA-256 from before the
   test, so the content is byte-identical. Limit: WirePlumber keeps this table
   in memory and it was not restarted, so it can write the two lines again the
   next time it saves this file. The lines name sinks that no longer exist and
   have no effect.
2. The built-in node `Dummy-Driver` was `idle` instead of `suspended`.
   `pw-cli send-command <id> Suspend '{}'` returned it to `suspended`.

Removed from the test machine: the null sink, the directory
`/srv/junto-spike/`, the image `junto-spike-build:1`, the six build cache
records of that image, and two helper scripts in `/tmp`. The containers
`junto-spike-cargo` and `junto-spike-probe` ran with `--rm`.

## What the product needs

1. One capture stream and links made with `link-factory`, kept up to date from
   registry events, as in this spike. No session manager policy, no portal and
   no external tool is needed for the capture.
2. A list of applications that play sound, built from the `Client` objects of
   the `Stream/Output/Audio` nodes. Group by `pipewire.access.portal.app_id`
   when it is set, otherwise by `application.process.binary` and
   `application.name`. Do not use `application.process.id` as the identity:
   sandboxed applications report the id inside their sandbox, and an
   application with several processes has several ids. Applications that play
   through a helper process were not tested.
3. Own playback excluded by the client id of the capture connection and by
   `pipewire.sec.pid`. If Junto plays through the PulseAudio layer, its nodes
   have the `pipewire.sec.pid` of `pipewire-pulse` and this rule does not find
   them; play through the same PipeWire connection, or mark the own nodes with
   a property.
4. A continuous time line: the callback does not run while no captured node
   runs, so the sender must insert silence from a clock.
5. Variable block size. The quantum was 256, 1024 and 2048 frames and changed
   during a run. Request `node.latency` on the capture stream when a short
   block is needed; the request lowers the quantum of every node on the same
   driver.
6. In all-system mode, skip nodes that have `node.link-group` (loopback and
   filter nodes), or the sound that passes through them is captured twice.
7. Channel handling for layouts other than stereo and mono. The spike links
   only FL, FR and MONO ports.
8. Accept that the first block of a new stream can be missing (1 of 8 starts).
9. A real-time-safe hand-over from the callback (a preallocated ring buffer).
   The spike allocates in the callback and uses a channel.
10. libpipewire as an optional run-time dependency. The spike binary links to
    `libpipewire-0.3.so.0` and does not start without it. A system without
    PipeWire needs another path.
11. For a Flatpak build: the permission for the PipeWire socket, and a test
    with a real build on more than one session manager configuration.

Not verified, to check before the product relies on them:

- The level of the captured audio against the stream volume and the sink
  volume. The capture point is the output port of the stream node, so the
  stream's own volume should apply and the sink volume should not (reasoned).
- The effect of linking streams that play to different output devices into
  one capture node (they then share one driver).
- How desktop mixers and recording indicators show the capture stream.
- Wayland sessions, a real Flatpak build, an AppImage build, JACK clients,
  PipeWire versions other than 1.6.8, session managers other than WirePlumber
  0.5.15.

## Dependencies and licences

- `pipewire` 0.10.1, `libspa` 0.10.1, `pipewire-sys` 0.10.1, `libspa-sys`
  0.10.1: MIT (read from each crate's `Cargo.toml`).
- `libc`: MIT OR Apache-2.0.
- All 59 dependency packages in `Cargo.lock`, including the build-time ones (`bindgen`,
  `clang-sys` and others), from `cargo metadata --locked`: MIT, Apache-2.0,
  MIT OR Apache-2.0, Apache-2.0 WITH LLVM-exception, BSD-3-Clause (`bindgen`),
  ISC (`libloading`), Unlicense OR MIT, and Unicode-3.0 together with MIT OR
  Apache-2.0 (`unicode-ident`). No GPL or LGPL package.
- PipeWire (the system library, linked dynamically): MIT.
- Source read for the API: the MIT examples of the `pipewire` crate
  (`audio-capture.rs`, `tone.rs`, `pw-mon.rs`,
  `create-delete-remote-objects.rs`). No GPL code was read or copied.

## What was left in place

- Test machine: nothing from this spike. Containers and images of other
  spikes with the prefix `junto-spike-` were present and were not touched.
- This directory: the source (`Cargo.toml`, `Cargo.lock`, `src/`), the build
  image definition (`Dockerfile.build`), the scripts (`scripts/`), this file,
  and `out/` with the logs (not tracked). There is no `target/` directory here;
  the build ran only on the test machine.
