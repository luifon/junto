#!/bin/sh
# Short check of the layout page: alice shares from the start, bob starts a share
# while the composite recording runs. Prints the tone levels of the recording.
set -eu
cd "$(dirname "$0")/.."
set -a; . ./.env.local; set +a
TP=../test-participant/target/release/test-participant
ROOM=${1:-short}
mkdir -p out/logs
$TP publish --room "$ROOM" --identity alice --mic-hz 440 --camera --screen --screen-audio-hz 1000 \
  --screen-color 2048c8 --leave-after 50 > "out/logs/$ROOM-alice.jsonl" 2>&1 &
$TP publish --room "$ROOM" --identity bob --mic-hz 620 --camera --screen --screen-audio-hz 1370 \
  --screen-color c83030 --screen-after 28 --leave-after 50 > "out/logs/$ROOM-bob.jsonl" 2>&1 &
sleep 3
sed "s/ROOM/$ROOM/g" requests/composite.json > "out/$ROOM-composite.json"
ID=$(lk egress start --type room-composite "out/$ROOM-composite.json" 2>/dev/null | sed -n 's/^EgressID: \([^ ]*\).*/\1/p')
echo "egress $ID"
sleep 42
lk egress stop --id "$ID" >/dev/null 2>&1
wait
sleep 3
FILE=$(lk egress list --id "$ID" --json 2>/dev/null | python3 -I -c 'import json,sys; print(json.load(sys.stdin)[0]["file_results"][0]["filename"].replace("/out/","out/"))')
echo "$FILE"
python3 -I scripts/tones.py levels "$FILE" --freqs 440,620,1000,1370 --window 2 --rows \
  | python3 -I -c 'import json,sys; [print(r) for r in json.load(sys.stdin)["rows"]]'
