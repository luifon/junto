#!/bin/bash
# Plays digital silence with each player and prints where the stream is linked and its identity properties.
. "$(dirname "$0")/env.sh"
RUN=$SPIKE_ROOT/run
mkdir -p "$RUN"; chown "$SPIKE_USER" "$RUN"
python3 - "$RUN/silence.wav" <<'PY'
import sys, wave
w = wave.open(sys.argv[1], "wb"); w.setnchannels(2); w.setsampwidth(2); w.setframerate(48000)
w.writeframes(b"\0" * (48000 * 4 * 4)); w.close()
PY
SINK=junto-spike-sink
PROPS='{ node.dont-fallback=true node.dont-reconnect=true state.restore-props=false state.restore-target=false }'
show() {
  as_user pw-dump | python3 -c '
import json, sys
objs = json.load(sys.stdin); by = {o["id"]: o for o in objs}
P = lambda o: (o.get("info") or {}).get("props") or {}
keys = ["node.name","media.class","client.id","client.api","application.name","application.process.id","application.process.binary","pipewire.sec.pid","pulse.server.type","target.object","node.dont-fallback","node.autoconnect","node.latency","object.serial","media.role","stream.is-live","node.rate","node.link-group"]
for o in objs:
    if o["type"].endswith(":Node") and "Stream" in str(P(o).get("media.class")):
        print("NODE", o["id"], {k: P(o).get(k) for k in keys if P(o).get(k) is not None}, "state", o["info"].get("state"))
        c = by.get(P(o).get("client.id"))
        if c: print("  CLIENT", c["id"], {k: v for k, v in P(c).items() if k.startswith(("application.", "pipewire.sec", "client.api", "pipewire.access", "pipewire.client.access"))})
        for q in objs:
            if q["type"].endswith(":Port") and P(q).get("node.id") == o["id"]:
                print("  PORT", q["id"], P(q).get("port.name"), q["info"].get("direction"), P(q).get("audio.channel"), P(q).get("format.dsp"))
for o in objs:
    if o["type"].endswith(":Link"):
        i = o["info"]; print("LINK", P(by[i["output-node-id"]]).get("node.name"), "->", P(by[i["input-node-id"]]).get("node.name"), i.get("state"))
'
}
echo "== pw-play"
as_user pw-play --target "$SINK" -P "$PROPS" "$RUN/silence.wav" & sleep 1.5; echo "host pid $(pgrep -n -x pw-play)"; show; wait
echo "== paplay"
as_user paplay --device="$SINK" --property=node.dont-fallback=true --property=state.restore-props=false --property=state.restore-target=false "$RUN/silence.wav" & sleep 1.5; echo "host pid $(pgrep -n -x paplay)"; show; wait
echo "== aplay"
as_user env PIPEWIRE_NODE="$SINK" PIPEWIRE_PROPS="$PROPS" aplay -q -D pipewire "$RUN/silence.wav" & sleep 1.5; echo "host pid $(pgrep -n -x aplay)"; show; wait
echo "== pw-play to a missing target with dont-fallback"
as_user timeout 3 pw-play --target junto-spike-missing -P "$PROPS" "$RUN/silence.wav" & sleep 1.5; show; wait
echo "== paplay to a missing device"
as_user timeout 3 paplay --device=junto-spike-missing --property=node.dont-fallback=true "$RUN/silence.wav"; echo "exit $?"
