#!/bin/bash
# Runs as the desktop user inside the run directory.
# A virtual sink made with pw-loopback forwards to the test sink, as equaliser and virtual-device setups do.
set -u
RUN=$(pwd); BIN=$RUN/junto-audio-spike; LOGS=$RUN/logs; SINK=junto-spike-sink; LOOP=junto-spike-loop-sink
python3 "$RUN/watch_real_sinks.py" "$LOGS/real-sinks-loopback.log" "$RUN/watch.stop" & WATCH=$!
pw-loopback --capture-props "{ media.class=Audio/Sink node.name=$LOOP node.description=\"junto spike loopback sink\" priority.session=0 audio.position=[FL,FR] }" \
  --playback-props "{ node.name=junto-spike-loop-out target.object=$SINK node.dont-fallback=true node.dont-reconnect=true state.restore-props=false state.restore-target=false audio.position=[FL,FR] }" & LOOPPID=$!
sleep 1.5
show() {
  pw-dump | python3 -c '
import json, sys
objs = json.load(sys.stdin); by = {o["id"]: o for o in objs}
P = lambda o: (o.get("info") or {}).get("props") or {}
for o in objs:
    if o["type"].endswith(":Node") and "junto-spike" in str(P(o).get("node.name")) or (o["type"].endswith(":Node") and "Stream" in str(P(o).get("media.class"))):
        print("NODE", o["id"], P(o).get("node.name"), P(o).get("media.class"), "node.link-group", P(o).get("node.link-group"), "client", P(o).get("client.id"))
for o in objs:
    if o["type"].endswith(":Link"):
        i = o["info"]; print("LINK", P(by[i["output-node-id"]]).get("node.name"), "->", P(by[i["input-node-id"]]).get("node.name"), i.get("state"))
'
}
echo "=== loopback alone (no sound playing)"; show | tee "$LOGS/loopback-idle.log"
echo "default sink: $(pw-metadata 0 default.audio.sink | grep key)"
if grep "^LINK" "$LOGS/loopback-idle.log" | grep -vE -- "-> ($SINK|junto-spike-loop-out) " | grep -q .; then echo "unexpected link; stopping"; kill $LOOPPID; touch "$RUN/watch.stop"; wait; exit 1; fi
for variant in plain skip; do
  extra=(); [ $variant = skip ] && extra=(--skip-link-groups)
  echo "=== system capture while pw-play (440 Hz) plays into the loopback sink; options: ${extra[*]:-none}"
  pw-play --target "$LOOP" -P '{ node.dont-fallback=true node.dont-reconnect=true state.restore-props=false state.restore-target=false }' "$RUN/tone-a.wav" & A=$!
  sleep 1.5
  "$BIN" capture --mode system --seconds 5 --out "$RUN/loopback-$variant.f32" "${extra[@]}" > "$LOGS/loopback-$variant.capture.log" 2>&1 & CAP=$!
  sleep 2; show > "$LOGS/loopback-$variant.streams.log"; wait $CAP
  kill $A; wait $A 2>/dev/null
  grep -E "decision|error" "$LOGS/loopback-$variant.capture.log" | cut -c1-330
  "$BIN" analyze --in "$RUN/loopback-$variant.f32" --freqs 440,1000 --ranges 1-4 | tee "$LOGS/loopback-$variant.analysis.log"
done
kill $LOOPPID; wait $LOOPPID 2>/dev/null
touch "$RUN/watch.stop"; wait "$WATCH"; rm -f "$RUN/watch.stop"
echo "real-sink watcher: $(tail -1 "$LOGS/real-sinks-loopback.log")"
