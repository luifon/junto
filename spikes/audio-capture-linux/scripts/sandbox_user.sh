#!/bin/bash
# Runs as the desktop user inside the run directory.
# Starts programs in a bubblewrap sandbox with its own process-id namespace and a /.flatpak-info file,
# which is the file PipeWire reads to classify a client as a Flatpak application.
# This is not a real Flatpak build. It needs no Flatpak service and leaves nothing behind.
set -u
RUN=$(pwd)
BIN=$RUN/junto-audio-spike
LOGS=$RUN/logs
SINK=junto-spike-sink
NOFALLBACK='{ node.dont-fallback=true node.dont-reconnect=true state.restore-props=false state.restore-target=false }'
FREQS=440,1000,1700,2500,3300
RUNTIME=$XDG_RUNTIME_DIR

sandbox() {
  local app_id=$1; shift
  printf '[Application]\nname=%s\nruntime=runtime/org.freedesktop.Platform/x86_64/24.08\n\n[Instance]\ninstance-id=%s\n\n[Context]\nsockets=pulseaudio;\nfilesystems=xdg-run/pipewire-0;\n' "$app_id" "$RANDOM$RANDOM" > "$RUN/flatpak-info-$app_id"
  bwrap --unshare-pid --die-with-parent \
    --ro-bind /usr /usr --symlink usr/lib64 /lib64 --symlink usr/lib /lib --symlink usr/bin /bin --symlink usr/sbin /sbin \
    --ro-bind /etc /etc --proc /proc --dev /dev --tmpfs /tmp \
    --dir "$RUNTIME" --bind "$RUNTIME/pipewire-0" "$RUNTIME/pipewire-0" --bind "$RUNTIME/pulse" "$RUNTIME/pulse" \
    --bind "$RUN" "$RUN" --ro-bind "$RUN/flatpak-info-$app_id" /.flatpak-info \
    --setenv XDG_RUNTIME_DIR "$RUNTIME" --chdir "$RUN" "$@"
}
snapshot() {
  pw-dump | python3 -c '
import json, sys
objs = json.load(sys.stdin); by = {o["id"]: o for o in objs}
P = lambda o: (o.get("info") or {}).get("props") or {}
for o in objs:
    if o["type"].endswith(":Node") and "Stream" in str(P(o).get("media.class")):
        c = P(by.get(P(o).get("client.id"), {}))
        print("STREAM node", o["id"], P(o).get("node.name"), "| client", P(o).get("client.id"), {k: v for k, v in c.items() if k.startswith(("pipewire.sec", "pipewire.access", "pipewire.client.access", "application.process.id", "application.process.binary", "application.name", "client.api"))})
for o in objs:
    if o["type"].endswith(":Link"):
        i = o["info"]; print("LINK", P(by[i["output-node-id"]]).get("node.name"), "->", P(by[i["input-node-id"]]).get("node.name"), i.get("state"))
'
}
stop() { for pid in "$@"; do kill "$pid" 2>/dev/null; done; wait "$@" 2>/dev/null; }
python3 "$RUN/watch_real_sinks.py" "$LOGS/real-sinks-sandbox.log" "$RUN/watch.stop" & WATCH=$!

echo "=== step 1: routing check with silence from inside the sandbox"
sandbox org.example.SandboxedNative pw-play --target "$SINK" -P "$NOFALLBACK" "$RUN/silence.wav" & S1=$!
sandbox org.example.SandboxedPulse paplay --device="$SINK" --property=node.dont-fallback=true --property=state.restore-props=false --property=state.restore-target=false "$RUN/silence.wav" & S2=$!
sleep 2; snapshot | tee "$LOGS/sandbox-routing.log"; wait "$S1" "$S2"
if grep -E "^LINK" "$LOGS/sandbox-routing.log" | grep -v -- "-> $SINK" | grep -q .; then echo "a sandboxed stream is linked to something other than the test sink; stopping"; touch "$RUN/watch.stop"; wait "$WATCH"; exit 1; fi
grep -q -- "-> $SINK" "$LOGS/sandbox-routing.log" || echo "note: no sandboxed stream reached the test sink"

echo "=== step 2: capture program inside the sandbox, system mode, own tone 2500 Hz; A = pw-play 440 Hz outside, B = paplay 1000 Hz inside another sandbox"
pw-play --target "$SINK" -P "$NOFALLBACK" "$RUN/tone-a.wav" & A=$!
sandbox org.example.SandboxedPulse paplay --device="$SINK" --property=node.dont-fallback=true --property=state.restore-props=false --property=state.restore-target=false "$RUN/tone-b.wav" & B=$!
sleep 1.5
sandbox org.example.JuntoSpike "$BIN" capture --mode system --self-tone 2500 --self-target "$SINK" --seconds 6 --out "$RUN/sandbox-system.f32" > "$LOGS/sandbox-system.capture.log" 2>&1 & CAP=$!
sleep 2.5; snapshot | tee "$LOGS/sandbox-system.streams.log"
wait "$CAP"; echo "capture exit status $?"
grep -E "CAPTURE|decision|link_|error|STATS callbacks" "$LOGS/sandbox-system.capture.log"
[ -s "$RUN/sandbox-system.f32" ] && "$BIN" analyze --in "$RUN/sandbox-system.f32" --freqs "$FREQS" --ranges 1-5 | tee "$LOGS/sandbox-system.analysis.log"

echo "=== step 3: capture program outside the sandbox, app mode, selecting the sandboxed paplay by pipewire.access.portal.app_id"
"$BIN" capture --mode app --portal-app-id org.example.SandboxedPulse --seconds 5 --out "$RUN/sandbox-app.f32" > "$LOGS/sandbox-app.capture.log" 2>&1
grep -E "decision|link_|STATS callbacks" "$LOGS/sandbox-app.capture.log"
"$BIN" analyze --in "$RUN/sandbox-app.f32" --freqs "$FREQS" --ranges 1-4 | tee "$LOGS/sandbox-app.analysis.log"
stop "$A" "$B"
touch "$RUN/watch.stop"; wait "$WATCH"; rm -f "$RUN/watch.stop" "$RUN"/flatpak-info-*
echo "real-sink watcher: $(tail -1 "$LOGS/real-sinks-sandbox.log")"
