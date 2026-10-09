#!/bin/sh
# Token lifetime facts. Output in out/m7/. Tokens are stored in out/m7/*.jwt and never printed.
here=$(cd "$(dirname "$0")" && pwd); out="$here/../out/m7"; mkdir -p "$out"; rm -f "$out"/*
py="$here/../out/venv/bin/python"
t0=$(date +%s)
log() { echo "[t+$(( $(date +%s) - t0 ))s] $*" | tee -a "$out/timeline.txt"; }
p() { "$here/lk-env.sh" "$py" "$here/participant.py" "$@" 2>/dev/null; }
short() { grep -E '"event": "(token_loaded|token_minted|token_state_at_connect|connected|connect_failed|token_refreshed|disconnected|left)"' "$1" | cut -c1-230 | sed 's/^/      /'; }

log "1. token valid 60 s, first connection attempt after 70 s"
p --room m7-late --identity late --ttl 60 --wait-before-connect 70 --duration 5 > "$out/late.jsonl" &
late_pid=$!

log "2. join with a 60 s token, stay connected, then RemoveParticipant"
p --room m7 --identity x --ttl 60 --save-token "$out/original.jwt" --save-refreshed "$out/refreshed.jwt" --duration 200 > "$out/x-first.jsonl" &
sleep 10
log "RemoveParticipant: $("$here/lk-env.sh" lk room participants remove --room m7 x 2>&1 | grep -v 'Using url' | tr '\n' ' ')"
sleep 3
short "$out/x-first.jsonl" | tee -a "$out/timeline.txt"

log "3a. rejoin with the ORIGINAL token, inside its 60 s lifetime"
p --room m7 --identity x --token-file "$out/original.jwt" --duration 6 > "$out/x-rejoin-original.jsonl"
short "$out/x-rejoin-original.jsonl" | tee -a "$out/timeline.txt"
log "participants after 3a: $("$here/lk-env.sh" lk room participants list m7 2>&1 | grep -v 'Using url' | tr '\n' ' ')"

wait $late_pid
log "result of 1:"; short "$out/late.jsonl" | tee -a "$out/timeline.txt"

log "3b. the original token has expired; rejoin with the original token (control)"
p --room m7 --identity x --token-file "$out/original.jwt" --duration 4 > "$out/x-rejoin-original-expired.jsonl"
short "$out/x-rejoin-original-expired.jsonl" | tee -a "$out/timeline.txt"
log "3c. rejoin with the REFRESHED token the server sent to the client before the removal"
p --room m7 --identity x --token-file "$out/refreshed.jwt" --save-refreshed "$out/refreshed-2.jwt" --duration 20 > "$out/x-rejoin-refreshed.jsonl" &
sleep 8
log "participants during 3c: $("$here/lk-env.sh" lk room participants list m7 2>&1 | grep -v 'Using url' | tr '\n' ' ')"
log "RemoveParticipant again: $("$here/lk-env.sh" lk room participants remove --room m7 x 2>&1 | grep -v 'Using url' | tr '\n' ' ')"
wait
short "$out/x-rejoin-refreshed.jsonl" | tee -a "$out/timeline.txt"
log "4. a participant that joined with a 60 s token and stays connected past 60 s"
p --room m7-stay --identity stay --ttl 60 --duration 75 --stats-every 30 > "$out/stay.jsonl"
short "$out/stay.jsonl" | tee -a "$out/timeline.txt"
log "done"
