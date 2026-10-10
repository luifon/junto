#!/bin/sh
# Runs inside the worker container started with --network none.
device="$1"
trace="strace -f -qq -e trace=socket,connect -o "
[ "${4:-trace}" = notrace ] && trace="env TRACE_FILE="
echo "## environment"
env | grep -E "^(HF_HUB_OFFLINE|HF_HUB_DISABLE_TELEMETRY|DO_NOT_TRACK|ORT_DISABLE_TELEMETRY)=" | sort
echo "## network interfaces: $(ls /sys/class/net | tr '\n' ' ')"
python - <<'PY'
import socket
for target in (("1.1.1.1", 443), ("huggingface.co", 443)):
    try:
        socket.create_connection(target, 3).close()
        print("connection to", target, "SUCCEEDED")
    except OSError as error:
        print("connection to", target, "failed:", error)
PY
echo "## microphone track ($device)"
: > /tmp/mic.trace
$trace/tmp/mic.trace \
  python worker.py --kind mic --audio /audio/silence_quiet.wav --reference /audio/silence_quiet.ref.json \
  --device "$device" --glossary /texts/glossary_prompt.txt --member "Member A" --stats "/out/t6_mic_$device.json"
echo "## shared-sound track, compressed twice ($device)"
: > /tmp/shared.trace
$trace/tmp/shared.trace \
  python worker.py --kind shared --audio /audio/diar_en_opus2.wav --reference /audio/diar_en_opus2.ref.json \
  --device "$device" --glossary /texts/glossary_prompt.txt --embedding "$2" --threshold "$3" --stats "/out/t6_shared_$device.json"
echo "## socket and connect system calls during both runs (tracing: ${4:-trace})"
for trace in /tmp/mic.trace /tmp/shared.trace; do
  echo "$trace: total $(grep -c -E 'socket\(|connect\(' $trace), AF_INET or AF_INET6 $(grep -c -E 'AF_INET' $trace), AF_UNIX $(grep -c AF_UNIX $trace), AF_NETLINK $(grep -c AF_NETLINK $trace)"
  grep -E "AF_INET" $trace | cut -c1-160 | head -5
done
echo "## files created outside /out and /tmp"
find / -xdev -newer /tmp/mic.trace -type f 2>/dev/null | grep -v -E "^/(proc|sys|dev|out|tmp)/|__pycache__" | head -20
ls -la /root/.cache 2>/dev/null | head -5
echo "## done"
