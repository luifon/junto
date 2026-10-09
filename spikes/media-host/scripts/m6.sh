#!/bin/sh
# Redis outage during a call. Output in out/m6/. JUNTO_TEST_MEDIA_HOST is the SSH name of the media host.
media_host="${JUNTO_TEST_MEDIA_HOST:?set the SSH name of the public host}"
here=$(cd "$(dirname "$0")" && pwd); out="$here/../out/m6"; mkdir -p "$out"; rm -f "$out"/*
py="$here/../out/venv/bin/python"
t0=$(date +%s)
log() { echo "[t+$(( $(date +%s) - t0 ))s] $*" | tee -a "$out/timeline.txt"; }
join() { "$here/lk-env.sh" "$py" "$here/participant.py" --room m6 --identity "$1" --publish both --duration "$2" > "$out/$1.jsonl" 2>/dev/null; }
api() { log "API $1: $("$here/lk-env.sh" lk room $2 2>&1 | grep -v 'Using url' | tr '\n' ' ' | cut -c1-300)"; }

join m6-a 150 & join m6-b 150 &
sleep 20
api "before outage, list rooms" "list"
log "docker stop junto-spike-redis: $(ssh "$media_host" 'docker stop junto-spike-redis')"
sleep 10
log "joining m6-c while Redis is down"
join m6-c 15; log "m6-c result: $(grep -E '"event": "(connected|connect_failed)"' "$out/m6-c.jsonl" | cut -c1-260)"
api "during outage, list rooms" "list"
api "during outage, list participants" "participants list m6"
sleep 15
log "docker start junto-spike-redis: $(ssh "$media_host" 'docker start junto-spike-redis')"
sleep 3
api "3 s after restart, list rooms" "list"
log "joining m6-d after restart"
join m6-d 15; log "m6-d result: $(grep -E '"event": "(connected|connect_failed)"' "$out/m6-d.jsonl" | cut -c1-260)"
api "after restart, list participants" "participants list m6"
wait
log "done"
