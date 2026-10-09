#!/bin/bash
# Usage: sink.sh create|destroy|id   Manages the test null sink "junto-spike-sink".
. "$(dirname "$0")/env.sh"
SINK=junto-spike-sink
sink_ids() {
  as_user pw-dump | python3 -c '
import json, sys
for o in json.load(sys.stdin):
    p = (o.get("info") or {}).get("props") or {}
    if o["type"].endswith(":Node") and p.get("node.name") == sys.argv[1]:
        print(o["id"])
' "$SINK"
}
case "$1" in
  create)
    if [ -n "$(sink_ids)" ]; then echo "sink exists: $(sink_ids)"; exit 0; fi
    before=$(as_user pw-metadata 0 default.audio.sink | grep "key:")
    as_user pw-cli create-node adapter "{ factory.name=support.null-audio-sink node.name=$SINK node.description=\"junto spike test sink\" media.class=Audio/Sink object.linger=true audio.position=[FL,FR] priority.session=0 priority.driver=0 }"
    sleep 1
    after=$(as_user pw-metadata 0 default.audio.sink | grep "key:")
    if [ "$before" != "$after" ]; then
      echo "DEFAULT SINK CHANGED, removing test sink"; echo "$before"; echo "$after"
      for id in $(sink_ids); do as_user pw-cli destroy "$id"; done
      exit 1
    fi
    echo "created sink id $(sink_ids); default sink unchanged"
    ;;
  destroy)
    for id in $(sink_ids); do as_user pw-cli destroy "$id"; echo "destroyed $id"; done
    ;;
  id) sink_ids ;;
esac
