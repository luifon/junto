#!/bin/bash
# Runs as the desktop user inside the run directory. Every player targets the test sink and refuses any fallback.
set -u
RUN=$(pwd)
BIN=$RUN/junto-audio-spike
LOGS=$RUN/logs
SINK=junto-spike-sink
NOFALLBACK='{ node.dont-fallback=true node.dont-reconnect=true state.restore-props=false state.restore-target=false }'
FREQS=440,1000,1700,2500,3300

pw-dump | grep -q "\"node.name\": \"$SINK\"" || { echo "test sink missing"; exit 1; }

play_native() { pw-play --target "$SINK" -P "$NOFALLBACK" "$1" & }
play_pulse()  { paplay --device="$SINK" --property=node.dont-fallback=true --property=node.dont-reconnect=true --property=state.restore-props=false --property=state.restore-target=false "$1" & }
play_alsa()   { PIPEWIRE_NODE="$SINK" PIPEWIRE_PROPS="$NOFALLBACK" aplay -q -D pipewire "$1" & }
stop() { for pid in "$@"; do kill "$pid" 2>/dev/null; done; wait "$@" 2>/dev/null; }
analyze() { "$BIN" analyze --in "$1" --freqs "$FREQS" "${@:2}"; }
snapshot_streams() {
  pw-dump | python3 -c '
import json, sys
objs = json.load(sys.stdin); by = {o["id"]: o for o in objs}
P = lambda o: (o.get("info") or {}).get("props") or {}
for o in objs:
    if o["type"].endswith(":Node") and "Stream" in str(P(o).get("media.class")):
        c = P(by.get(P(o).get("client.id"), {}))
        print("STREAM node", o["id"], P(o).get("media.class"), "node.name", P(o).get("node.name"), "| client", P(o).get("client.id"),
              "application.process.id", c.get("application.process.id"), "pipewire.sec.pid", c.get("pipewire.sec.pid"),
              "binary", c.get("application.process.binary"), "application.name", c.get("application.name"), "client.api", c.get("client.api"), "access", c.get("pipewire.access"))
for o in objs:
    if o["type"].endswith(":Link"):
        i = o["info"]; pr = P(o)
        print("LINK", o["id"], P(by[i["output-node-id"]]).get("node.name"), "->", P(by[i["input-node-id"]]).get("node.name"), i.get("state"), "created-by-client", pr.get("client.id"), "linger", pr.get("object.linger"))
'
}

start_watch() { rm -f "$RUN/watch.stop"; python3 "$RUN/watch_real_sinks.py" "$LOGS/real-sinks-$1.log" "$RUN/watch.stop" & WATCH=$!; sleep 0.5; }
stop_watch() { touch "$RUN/watch.stop"; wait "$WATCH"; rm -f "$RUN/watch.stop"; echo "real-sink watcher: $(tail -1 "$LOGS/real-sinks-$1.log")"; }

test_l1() {
  echo "=== L1 app-only capture (A = pw-play 440 Hz native, B = paplay 1000 Hz pulse, D = aplay 1700 Hz ALSA)"
  play_native "$RUN/tone-a.wav"; A=$!
  play_pulse "$RUN/tone-b.wav"; B=$!
  play_alsa "$RUN/tone-d.wav"; D=$!
  sleep 1.5
  echo "pids: A=$A B=$B D=$D"
  snapshot_streams
  local which pid
  for which in a b d; do
    case $which in a) pid=$A;; b) pid=$B;; d) pid=$D;; esac
    echo "--- capture of process $which (pid $pid)"
    "$BIN" capture --mode app --pid "$pid" --seconds 6 --out "$RUN/l1-$which.f32" | tee "$LOGS/l1-$which.capture.log" | grep -E "decision|link_requested|capture_format|STATS callbacks|STATS frames"
    analyze "$RUN/l1-$which.f32" --ranges 1-5 | tee "$LOGS/l1-$which.analysis.log"
  done
  stop "$A" "$B" "$D"
}

test_l2() {
  local label=$1; shift
  echo "=== L2 ($label) system capture, own tone 2500 Hz, extra options: $*"
  play_native "$RUN/tone-a.wav"; A=$!
  play_pulse "$RUN/tone-b.wav"; B=$!
  sleep 1
  pw-record --target "$SINK" -P '{ stream.capture.sink=true node.dont-fallback=true state.restore-props=false state.restore-target=false }' --raw --format f32 --rate 48000 --channels 2 "$RUN/$label-sink-monitor.f32" & MON=$!
  "$BIN" capture --mode system --self-tone 2500 --self-target "$SINK" --seconds 15 --out "$RUN/$label.f32" "$@" > "$LOGS/$label.capture.log" & CAP=$!
  sleep 2; snapshot_streams > "$LOGS/$label.streams-at-2s.log"
  sleep 3; echo "t~5s: starting late stream E (aplay 3300 Hz)"; play_alsa "$RUN/tone-e.wav"; E=$!
  sleep 5; echo "t~10s: stopping stream B (paplay 1000 Hz)"; stop "$B"
  wait "$CAP"
  stop "$MON" "$A" "$E"
  grep -E "CAPTURE|decision|link_requested|removed|capture_format|STATS" "$LOGS/$label.capture.log"
  cat "$LOGS/$label.streams-at-2s.log"
  echo "--- our capture"
  analyze "$RUN/$label.f32" --ranges 1-4.5,6.5-9.5,11.5-14.5 | tee "$LOGS/$label.analysis.log"
  analyze "$RUN/$label.f32" --window 0.5 > "$LOGS/$label.analysis-windows.log"
  echo "--- sink monitor recorded by pw-record during the same run (contains everything played into the sink)"
  analyze "$RUN/$label-sink-monitor.f32" --ranges 1-3 | tee "$LOGS/$label-sink-monitor.analysis.log"
}

test_l3_same_process() {
  echo "=== L3a same process, new node per cycle (helper 440 Hz, 3 cycles of 3 s with 1.5 s gaps), distractor B = paplay 1000 Hz"
  play_pulse "$RUN/tone-b.wav"; B=$!
  "$BIN" play --freq 440 --cycles 3 --seconds 3 --gap-seconds 1.5 --target "$SINK" --app-name junto-spike-app-a > "$LOGS/l3a.player.log" & A=$!
  sleep 0.7
  "$BIN" capture --mode app --pid "$A" --seconds 14 --out "$RUN/l3a.f32" > "$LOGS/l3a.capture.log"
  wait "$A"; stop "$B"
  cat "$LOGS/l3a.player.log"
  grep -E "decision|link_requested|removed|STATS gap|STATS callbacks" "$LOGS/l3a.capture.log"
  analyze "$RUN/l3a.f32" --window 0.5 | tee "$LOGS/l3a.analysis-windows.log"
}

test_l3_new_process() {
  echo "=== L3b new process each time, matched by application.process.binary=aplay (1700 Hz), distractor B = paplay 1000 Hz"
  play_pulse "$RUN/tone-b.wav"; B=$!
  "$BIN" capture --mode app --binary aplay --seconds 13 --out "$RUN/l3b.f32" > "$LOGS/l3b.capture.log" & CAP=$!
  sleep 2; play_alsa "$RUN/tone-d.wav"; D1=$!; echo "first aplay pid $D1"; sleep 3; stop "$D1"
  sleep 2; play_alsa "$RUN/tone-d.wav"; D2=$!; echo "second aplay pid $D2"; sleep 3; stop "$D2"
  wait "$CAP"; stop "$B"
  grep -E "decision|link_requested|removed|STATS gap|STATS callbacks" "$LOGS/l3b.capture.log"
  analyze "$RUN/l3b.f32" --window 0.5 | tee "$LOGS/l3b.analysis-windows.log"

  echo "=== L3c new process each time, matched by application.name=junto-spike-app-a (helper 440 Hz), distractor B = paplay 1000 Hz"
  play_pulse "$RUN/tone-b.wav"; B=$!
  "$BIN" capture --mode app --app-name junto-spike-app-a --seconds 13 --out "$RUN/l3c.f32" > "$LOGS/l3c.capture.log" & CAP=$!
  sleep 2; "$BIN" play --freq 440 --seconds 3 --target "$SINK" --app-name junto-spike-app-a > "$LOGS/l3c.player1.log"
  sleep 2; "$BIN" play --freq 440 --seconds 3 --target "$SINK" --app-name junto-spike-app-a > "$LOGS/l3c.player2.log"
  wait "$CAP"; stop "$B"
  cat "$LOGS/l3c.player1.log" "$LOGS/l3c.player2.log"
  grep -E "decision|link_requested|removed|STATS gap|STATS callbacks" "$LOGS/l3c.capture.log"
  analyze "$RUN/l3c.f32" --window 0.5 | tee "$LOGS/l3c.analysis-windows.log"
}

latency_case() {
  local label=$1 capture_latency=$2 with_pulse_player=$3
  echo "--- L4 case $label (capture node.latency: ${capture_latency:-not set}; paplay also playing: $with_pulse_player)"
  extra=(); [ -n "$capture_latency" ] && extra=(--latency "$capture_latency")
  [ "$with_pulse_player" = yes ] && { play_pulse "$RUN/tone-b.wav"; B=$!; }
  "$BIN" capture --mode app --app-name junto-spike-latency --seconds 7 --out "$RUN/$label.f32" "${extra[@]}" > "$LOGS/$label.capture.log" & CAP=$!
  sleep 1
  "$BIN" play --freq 440 --lead-silence 2 --seconds 2.5 --target "$SINK" --app-name junto-spike-latency > "$LOGS/$label.player.log" & P=$!
  sleep 3; pw-top -b -n 2 2>/dev/null | tail -12 > "$LOGS/$label.pw-top.log"
  wait "$P" "$CAP"; [ "$with_pulse_player" = yes ] && stop "$B"
  grep -E "junto|QUANT" "$LOGS/$label.pw-top.log"
  grep PLAYER "$LOGS/$label.player.log"
  grep -E "capture_format|STATS" "$LOGS/$label.capture.log"
  python3 - "$LOGS/$label.player.log" "$LOGS/$label.capture.log" <<'PY'
import re, sys
player = open(sys.argv[1]).read(); capture = open(sys.argv[2]).read()
p = re.search(r"onset_ns=(\d+) onset_frame_in_block=(\d+) onset_block_frames=(\d+)", player)
c = re.search(r"STATS onset callback_ns=(\d+) frame_in_block=(\d+) block_frames=(\d+)", capture)
if p and c:
    print(f"LATENCY player_callback_to_capture_callback_ms={(int(c.group(1)) - int(p.group(1))) / 1e6:.3f} "
          f"player_frame={p.group(2)}/{p.group(3)} capture_frame={c.group(2)}/{c.group(3)} "
          f"block_ms={int(c.group(3)) / 48:.2f}")
else:
    print("LATENCY onset not found")
PY
}

test_l4() {
  echo "=== L4 format and latency"
  pw-metadata -n settings | grep clock
  latency_case l4-default "" no
  latency_case l4-with-pulse-player "" yes
  latency_case l4-256 256/48000 no
  latency_case l4-256-with-pulse-player 256/48000 yes
}

for name in "$@"; do
  start_watch "$name"
  case $name in
    l1) test_l1 ;;
    l2) test_l2 l2-exclude-own ;;
    l2-control) test_l2 l2-include-own --include-self ;;
    l3a) test_l3_same_process ;;
    l3b) test_l3_new_process ;;
    l4) test_l4 ;;
  esac
  stop_watch "$name"
done
