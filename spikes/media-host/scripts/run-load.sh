#!/bin/sh
# usage: [SECONDGEN="<lk load-test options run in a container on the second machine, with packet counters>"] [HOSTGEN="<lk load-test options run in a container on the media host>"] [PROCS=<n parallel lk processes, each with the given options>] [PUBLISH_FILE=<h264 file> PUBLISH_FPS=<n>] run-load.sh <label> <seconds> <lk load-test options...>
# JUNTO_TEST_MEDIA_HOST is the SSH name of the media host. JUNTO_TEST_DATA_HOST is the SSH name of the second machine and is needed with SECONDGEN.
set -u
media_host="${JUNTO_TEST_MEDIA_HOST:?set the SSH name of the public host}"
[ -z "${SECONDGEN:-}" ] || second_host="${JUNTO_TEST_DATA_HOST:?set the SSH name of the data host}"
here=$(cd "$(dirname "$0")" && pwd)
out="$here/../out/m5"; mkdir -p "$out"
label=$1; seconds=$2; shift 2
iface=$(route -n get default | awk '/interface/{print $2}')
metrics() { ssh "$media_host" 'curl -s localhost:6789/metrics | grep -E "^livekit_(packet_total|bytes_total|nack_total|pli_total|packet_loss|forward_latency_(sum|count)|participant_total|track_(published|subscribed)_total)"'; }
drops() { echo "mac: $(netstat -s -p udp | grep -E "full socket buffers|datagrams received" | tr -s ' ' | tr '\n' ';')"; ssh "$media_host" 'i=$(ip -o -4 route get 1.1.1.1 | sed -n "s/.* dev \([^ ]*\).*/\1/p"); echo "host qdisc: $(tc -s qdisc show dev $i | grep -E "dropped" | head -1)"; echo "host tx_dropped=$(cat /sys/class/net/$i/statistics/tx_dropped) rx_dropped=$(cat /sys/class/net/$i/statistics/rx_dropped) rx_packets=$(cat /sys/class/net/$i/statistics/rx_packets) tx_packets=$(cat /sys/class/net/$i/statistics/tx_packets)"; awk "/^Udp:/{n++; if(n==2) print \"host udp4 InErrors=\" \$4 \" RcvbufErrors=\" \$6 \" SndbufErrors=\" \$7}" /proc/net/snmp; grep -E "Udp6(InErrors|RcvbufErrors|SndbufErrors)" /proc/net/snmp6 | tr -s " " | tr "\n" ";"; echo'; echo "mac iface packets in/out: $(netstat -ibn -I "$iface" | awk '/Link#/{print $5, $8}')"; }
metrics > "$out/$label.metrics-before.txt"
drops > "$out/$label.drops-before.txt"
ssh "$media_host" "sh /opt/junto-spike/sample.sh $((seconds + 10)) 5" > "$out/$label.host.txt" &
python3 "$here/mac-sample.py" $((seconds + 10)) 5 "$iface" lk > "$out/$label.mac.txt" &
( sleep $((seconds * 2 / 3)); ssh "$media_host" 'docker stats --no-stream --format "{{.Name}} cpu={{.CPUPerc}} mem={{.MemUsage}}" junto-spike-livekit junto-spike-redis; top -b -n 1 | head -12; free -m' > "$out/$label.dockerstats.txt" 2>&1; metrics > "$out/$label.metrics-during.txt" ) &
pub_pid=""
if [ -n "${PUBLISH_FILE:-}" ]; then
  "$here/lk-env.sh" lk room join --identity screen-publisher --publish "$PUBLISH_FILE" --fps "${PUBLISH_FPS:-15}" "m5-$label" > "$out/$label.publisher.txt" 2>&1 &
  pub_pid=$!
  sleep 3
fi
procs=${PROCS:-1}
: > "$out/$label.lk.txt"
gen_pids=""
i=1
while [ $i -le $procs ]; do
  "$here/lk-env.sh" lk load-test --room "m5-$label" --identity-prefix "g$i" --duration "${seconds}s" "$@" > "$out/$label.lk.$i.txt" 2>&1 &
  gen_pids="$gen_pids $!"
  i=$((i + 1))
done
if [ -n "${SECONDGEN:-}" ]; then
  ssh "$second_host" 'docker rm -f junto-spike-netns junto-spike-loadgen >/dev/null 2>&1; docker run -d --name junto-spike-netns --cap-add NET_ADMIN alpine:3.22 sh -c "apk add -q iptables >/dev/null 2>&1; iptables -A INPUT -p udp --sport 7882; iptables -A OUTPUT -p udp --dport 7882; touch /ready; sleep 900" >/dev/null; n=0; until docker exec junto-spike-netns test -f /ready 2>/dev/null || [ $n -gt 30 ]; do sleep 1; n=$((n+1)); done'
  "$here/lk-env.sh" sh -c 'printf "LIVEKIT_URL=%s\nLIVEKIT_API_KEY=%s\nLIVEKIT_API_SECRET=%s\n" "$LIVEKIT_URL" "$LIVEKIT_API_KEY" "$LIVEKIT_API_SECRET"' | ssh "$second_host" "docker run --rm --name junto-spike-loadgen --network container:junto-spike-netns --env-file /dev/stdin livekit/livekit-cli:v2.18.8 load-test --room m5-$label --identity-prefix second --duration ${seconds}s $SECONDGEN" > "$out/$label.lk.second.txt" 2>&1 &
  gen_pids="$gen_pids $!"
fi
if [ -n "${HOSTGEN:-}" ]; then
  ssh "$media_host" "cd /opt/junto-spike && set -a && . ./.env && export LIVEKIT_URL=ws://127.0.0.1:7880 LIVEKIT_API_KEY=\$LK_API_KEY LIVEKIT_API_SECRET=\$LK_API_SECRET && docker run --rm --name junto-spike-loadgen --network host -e LIVEKIT_URL -e LIVEKIT_API_KEY -e LIVEKIT_API_SECRET livekit/livekit-cli:v2.18.8 load-test --room m5-$label --identity-prefix hostgen --duration ${seconds}s $HOSTGEN" > "$out/$label.lk.hostgen.txt" 2>&1 &
  gen_pids="$gen_pids $!"
fi
wait $gen_pids
cat "$out/$label".lk.[0-9]*.txt >> "$out/$label.lk.txt" 2>/dev/null; rm -f "$out/$label".lk.[0-9]*.txt
[ -n "$pub_pid" ] && pkill -P "$pub_pid" 2>/dev/null; [ -n "$pub_pid" ] && kill "$pub_pid" 2>/dev/null; pkill -f "identity screen-publisher" 2>/dev/null
wait
metrics > "$out/$label.metrics-after.txt"
if [ -n "${SECONDGEN:-}" ]; then
  ssh "$second_host" 'docker exec junto-spike-netns iptables -L -v -n -x | grep -E "spt:7882|dpt:7882" | awk "{print \$NF, \"packets=\" \$1, \"bytes=\" \$2}"; docker rm -f junto-spike-netns >/dev/null' > "$out/$label.second-counters.txt" 2>&1
fi
drops > "$out/$label.drops-after.txt"
