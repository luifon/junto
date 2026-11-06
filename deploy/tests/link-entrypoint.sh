#!/bin/sh
# Tests the decisions of the start script of the link (deploy/images/link/entrypoint.sh) in the
# link image, with stand-ins for wireguard-go, wg, ip, date and curl (ADR 0003):
# - a read of wg that fails while a handshake exists does not end a link that works;
# - a restart writes the state of the loss and sends no notice; the next start sends it;
# - a loss ends at a renewal of the keys after the first handshake of a start, not at that
#   first handshake;
# - the address of the notices is not in the arguments of curl;
# - each forwarder keeps its stated bound and the others share the rest of the limit of
#   processes less a reserve; bounds that the limit does not hold end the start; a link at
#   that limit waits without a fork and keeps running.
# Takes about a minute. Everything is removed at the end.
#
#   deploy/tests/link-entrypoint.sh [<name prefix>]
set -eu
prefix=${1:-junto-link-entrypoint}
deploy=$(cd "$(dirname "$0")/.." && pwd)
work=$(mktemp -d "${TMPDIR:-/tmp}/$prefix.XXXXXX")
image=$prefix-image
container=$prefix-link
failures=0
remove() {
    code=$?
    docker rm -f "$container" > /dev/null 2>&1 || true
    docker image rm "$image" > /dev/null 2>&1 || true
    rm -rf "$work"
    exit "$code"
}
trap remove EXIT
trap 'exit 130' INT TERM

docker build -q -t "$image" "$deploy/images/link" > /dev/null
mkdir -p "$work/fake" "$work/state" "$work/keys"
chmod 777 "$work/state" "$work/keys"
echo "a-private-key" > "$work/keys/private"
cat > "$work/fake/wireguard-go" <<'EOF'
#!/bin/sh
mkdir -p /var/run/wireguard
exec socat UNIX-LISTEN:/var/run/wireguard/wg0.sock,fork,unlink-early /dev/null
EOF
# wg: "show" prints the handshake of /state/handshake, and fails once while /state/fail exists.
cat > "$work/fake/wg" <<'EOF'
#!/bin/sh
case "$1" in
    show)
        touch /state/read
        if [ -e /state/fail ]; then rm -f /state/fail; echo "failed" > /state/failed; exit 1; fi
        printf 'peer-key\t%s\n' "$(cat /state/handshake)" ;;
esac
exit 0
EOF
printf '#!/bin/sh\nexit 0\n' > "$work/fake/ip"
# date: the time of the system plus /state/offset seconds.
cat > "$work/fake/date" <<'EOF'
#!/bin/sh
if [ "$*" = "+%s" ]; then
    echo $(($(/bin/date +%s) + $(cat /state/offset)))
else
    exec /bin/date "$@"
fi
EOF
# curl: keeps its arguments and its configuration, one notice per file.
cat > "$work/fake/curl" <<'EOF'
#!/bin/sh
count=$(($(cat /state/count 2>/dev/null || echo 0) + 1))
echo "$count" > /state/count
echo "$*" > "/state/arguments-$count"
cat > "/state/notice-$count"
EOF
# socat: keeps the options of each listener and runs the real program.
cat > "$work/fake/socat" <<'EOF'
#!/bin/sh
case "$1" in TCP-LISTEN:*) echo "$1" >> /state/listeners ;; esac
exec /usr/bin/socat "$@"
EOF
chmod 755 "$work/fake/"*

check() {
    if eval "$2"; then
        echo "ok    $1"
    else
        echo "FAIL  $1"
        docker logs --tail 5 "$container" 2>&1 | sed 's/^/      /'
        failures=$((failures + 1))
    fi
}
now() { date +%s; }
offset() { echo "$1" > "$work/state/offset"; }
handshake() { echo "$1" > "$work/state/handshake"; }
# start [<options of docker run>...]
start() {
    docker rm -f "$container" > /dev/null 2>&1 || true
    rm -f "$work/state/read"
    docker run -d --name "$container" --user "$(id -u):$(id -g)" --network none --read-only "$@" \
        --tmpfs /var/run/wireguard:mode=1777 \
        -v "$work/fake:/fake:ro" -v "$work/state:/state" -v "$work/keys:/keys" \
        -e PATH=/fake:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
        -e LINK_ADDRESS=10.99.0.2 -e LINK_PEER_ADDRESS=10.99.0.1 -e LINK_PEER_PUBLIC_KEY=peer-key \
        -e LINK_NAME="data host" -e LINK_RESTART_AFTER=180 \
        -e LINK_NOTICE_URL="https://ntfy.example.test/secret-topic" \
        "$image" > /dev/null
}
running() { [ "$(docker inspect -f '{{.State.Running}}' "$container")" = true ]; }
# The script has passed its start and read wg once.
looping() { test -e "$work/state/read"; }
# until_true <seconds> <command...>: runs the command every half second until it succeeds.
until_true() {
    limit=$1
    shift
    deadline=$(($(date +%s) + limit))
    until "$@"; do
        [ "$(date +%s)" -lt "$deadline" ] || return 1
        sleep 0.5
    done
}
notices() { cat "$work/state/count" 2>/dev/null || echo 0; }
# reach <description> <seconds> <command...>: a step that the later checks need; the test
# ends with a failure when it is not reached.
reach() {
    description=$1
    shift
    if ! until_true "$@"; then
        echo "FAIL  $description"
        docker logs --tail 5 "$container" 2>&1 | sed 's/^/      /'
        exit 1
    fi
}

# A link that has run longer than its bound, with a handshake 5 seconds old, and one read of
# wg that fails.
offset 0
handshake "$(now)"
start
reach "the link reads its handshake" 10 looping
handshake "$(($(now) + 995))"
offset 1000
rm -f "$work/state/read"
reach "the link reads its new handshake" 10 looping
touch "$work/state/fail"
reach "the link meets the read that fails" 10 test -e "$work/state/failed"
rm -f "$work/state/read"
reach "the link reads again after the read that fails" 10 looping
check "a read of wg that fails does not end a link that works" "running && [ ! -e $work/keys/down ]"
check "it sends no notice" '[ "$(notices)" = 0 ]'

# The forwarders of the compose files in a container with their limit of processes: each
# stated bound, and the forwarder of the server takes the rest less 256.
docker rm -f "$container" > /dev/null
pids=$(awk '/^  link:/ { link = 1 } link && /pids_limit:/ { print $2; exit }' "$deploy/base.yaml" |
    sed -E 's/^\$\{[A-Z_]+:-([0-9]+)\}$/\1/')
bounds=$(sed -n 's|^        \$.*:\([0-9]*\)=.*/\([0-9]*\)$|\1 \2|p' "$deploy/two-hosts-data.yaml")
stated=$(echo "$bounds" | awk '{ sum += $2 } END { print sum }')
start --pids-limit "$pids" \
    -e LINK_FORWARDS="$(echo "$bounds" | awk '{ printf "127.0.0.1:%d=127.0.0.1:9/%d ", 7000 + NR, $2 }')127.0.0.1:7009=127.0.0.1:9"
reach "the link with the forwarders of the compose files reads its handshake" 10 looping
bounded() {
    [ "$(grep -c max-children= "$work/state/listeners")" -eq "$(($(echo "$bounds" | wc -l) + 1))" ] || return 1
    for bound in $(echo "$bounds" | awk '{ print $2 }'); do
        grep -q ",max-children=$bound," "$work/state/listeners" || return 1
    done
    grep -q "TCP-LISTEN:7009,.*,max-children=$((pids - 256 - stated))," "$work/state/listeners"
}
check "the forwarders keep their stated bounds ($(echo $bounds)) and the server forwarder takes the rest of $pids processes less 256" bounded
check "the server forwarder of the compose files has at least 1024 connections" \
    "[ $((pids - 256 - stated)) -ge 1024 ]"

# Stated bounds that the limit does not hold end the start.
docker rm -f "$container" > /dev/null
start --pids-limit 512 -e LINK_FORWARDS="127.0.0.1:7001=127.0.0.1:9/256 127.0.0.1:7002=127.0.0.1:9"
check "bounds that the limit of processes does not hold end the start with 2" \
    "until_true 10 sh -c '! docker inspect -f {{.State.Running}} $container | grep -q true' && [ \"\$(docker inspect -f '{{.State.ExitCode}}' $container)\" = 2 ] && docker logs $container 2>&1 | grep -q 'does not hold the connections of the forwarders'"

# A link at the limit of its processes: no fork, no read of wg, no end.
docker rm -f "$container" > /dev/null
offset 0
handshake "$(now)"
start --pids-limit "$pids"
reach "the link reads its handshake before its limit is lowered" 10 looping
docker update --pids-limit 2 "$container" > /dev/null
reach "the link states that it waits at its limit of processes" 10 \
    sh -c "docker logs $container 2>&1 | grep -q 'processes are free in the container; the link waits'"
rm -f "$work/state/read"
until_true 5 false || true
check "a link at its limit of processes keeps running without a read of wg" \
    "running && [ ! -e $work/state/read ] && [ ! -e $work/keys/down ]"
docker update --pids-limit "$pids" "$container" > /dev/null
check "it reads wg again when processes are free" "until_true 10 looping"
rm -f "$work/keys/down"

# No handshake for longer than the bound: the process ends, writes the state, sends nothing.
docker rm -f "$container" > /dev/null
offset 0
handshake 0
start
reach "the link without a handshake reads wg" 10 looping
offset 200
check "a link without a handshake for its bound ends" "until_true 10 sh -c '! docker inspect -f {{.State.Running}} $container | grep -q true'"
check "its process ends with 1" '[ "$(docker inspect -f "{{.State.ExitCode}}" "$container")" = 1 ]'
check "it writes the loss, not yet notified, and sends no notice" \
    '[ "$(cut -d " " -f 2,3 "$work/keys/down")" = "1 0" ] && [ "$(notices)" = 0 ]'

# The next start sends the notice of the loss; the first handshake does not end the loss, a
# renewal does.
offset 0
handshake "$(now)"
start
check "the next start sends the notice of the loss" "until_true 10 sh -c '[ \"\$(cat $work/state/count 2>/dev/null)\" = 1 ]'"
check "the notice states the loss" "grep -q 'link of the data host is down since' $work/state/notice-1"
# Two passes of the loop with the first handshake.
for pass in 1 2; do
    rm -f "$work/state/read"
    reach "the link reads the first handshake (pass $pass)" 10 looping
done
check "the first handshake of a start does not end the loss" '[ "$(notices)" = 1 ] && [ -s "$work/keys/down" ]'
handshake "$(($(now) + 1))"
check "a renewal of the keys ends the loss with a notice" "until_true 10 sh -c '[ \"\$(cat $work/state/count 2>/dev/null)\" = 2 ]'"
check "the notice states the return" "grep -q 'link of the data host is up again since' $work/state/notice-2"
check "the state of the loss is removed" "until_true 5 sh -c '[ ! -e $work/keys/down ]'"
check "the address is in the configuration of curl and not in its arguments" \
    "grep -q 'url = \"https://ntfy.example.test/secret-topic\"' $work/state/notice-1 && ! grep -q secret-topic $work/state/arguments-1"
check "the notice is plain text" "grep -q 'header = \"Content-Type: text/plain\"' $work/state/notice-1"

if [ "$failures" -eq 0 ]; then echo "all checks passed"; else echo "$failures check(s) failed"; exit 1; fi
