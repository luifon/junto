#!/bin/sh
# Tests the entry on port 443 (deploy/entry/Caddyfile, ADR 0004) without public DNS names:
# the entry signs its certificates itself, and three stand-in programs take the places of
# the server, the media server and its TURN port inside the network namespace of the entry.
# The test checks that each server name reaches its own program, that the routes of the
# server for the media server and the recorder are closed, and that the server gets the
# address of the client and not a forwarded address that the client states.
#
#   deploy/tests/entry.sh [<name prefix of the containers>]
set -eu
prefix=${1:-junto-entry-test}
deploy=$(cd "$(dirname "$0")/.." && pwd)
entry_image=$(sed -n 's/^ *image: \(livekit\/caddyl4:.*\)$/\1/p' "$deploy/base.yaml")
tools_image=busybox:1.37.0@sha256:bdf57e528e45e4433820e045b29b4597825a1c9e38353532d90a01445013f82e
names="chat.example.test livekit.example.test turn.example.test"

remove() {
    code=$?
    if [ "$code" -ne 0 ]; then
        docker logs --tail 30 "$prefix-entry" 2>&1 || true
    fi
    docker rm -f -v "$prefix-client" "$prefix-server" "$prefix-livekit" "$prefix-turn" "$prefix-entry" > /dev/null 2>&1 || true
    docker network rm "$prefix-network" > /dev/null 2>&1 || true
    exit "$code"
}
trap remove EXIT
trap 'exit 130' INT TERM

docker network create "$prefix-network" > /dev/null
hosts=""
for name in $names; do
    hosts="$hosts --add-host $name:127.0.0.1"
done
# shellcheck disable=SC2086
docker run -d --name "$prefix-entry" --network "$prefix-network" $hosts \
    --read-only --tmpfs /data --tmpfs /config --tmpfs /tmp \
    --cap-drop ALL --cap-add NET_BIND_SERVICE --security-opt no-new-privileges:true \
    --memory 128m --cpus 1 --pids-limit 128 \
    -e HOME=/data -e XDG_DATA_HOME=/data -e XDG_CONFIG_HOME=/config \
    -e JUNTO_ACME_EMAIL=admin@example.test -e JUNTO_ENTRY_CERTIFICATES=local_certs \
    -e JUNTO_SERVER_HOST=chat.example.test -e JUNTO_LIVEKIT_HOST=livekit.example.test \
    -e JUNTO_TURN_HOST=turn.example.test \
    -e JUNTO_ENTRY_SERVER_UPSTREAM=127.0.0.1:8080 -e JUNTO_ENTRY_LIVEKIT_UPSTREAM=127.0.0.1:7880 \
    -e JUNTO_ENTRY_TURN_UPSTREAM=127.0.0.1:5349 \
    -v "$deploy/entry/Caddyfile:/etc/caddy/Caddyfile:ro" \
    "$entry_image" run --config /etc/caddy/Caddyfile --adapter caddyfile > /dev/null

# A stand-in answers every request with its own name and keeps the last request.
stand_in() {
    docker run -d --name "$prefix-$1" --network "container:$prefix-entry" \
        --read-only --tmpfs /seen --cap-drop ALL --memory 32m --cpus 0.5 --pids-limit 32 \
        "$tools_image" sh -c "
            while true; do
                printf 'HTTP/1.0 200 OK\r\nContent-Type: text/plain\r\n\r\n$1-stand-in\n' | nc -l -s 127.0.0.1 -p $2 > /seen/next
                mv /seen/next /seen/request
            done" > /dev/null
}
stand_in server 8080
stand_in livekit 7880
stand_in turn 5349
docker run -d --name "$prefix-client" --network "container:$prefix-entry" \
    --read-only --tmpfs /tmp --cap-drop ALL --memory 32m --cpus 0.5 --pids-limit 32 \
    "$tools_image" sleep 600 > /dev/null

get() {
    docker exec "$prefix-client" wget -q -T 10 --no-check-certificate -O - "$@" 2>&1 || true
}

tries=0
until [ "$(get https://chat.example.test/ready)" = "server-stand-in" ]; do
    tries=$((tries + 1))
    if [ "$tries" -ge 60 ]; then
        echo "FAIL  the entry did not answer for the server name"
        exit 1
    fi
    sleep 0.5
done

failures=0
check() {
    if [ "$2" = "$3" ]; then
        echo "ok    $1"
    else
        echo "FAIL  $1: got \"$2\", expected \"$3\""
        failures=$((failures + 1))
    fi
}

check "the server name reaches the server" "$(get https://chat.example.test/api/server)" "server-stand-in"
check "the media server name reaches the media server" "$(get https://livekit.example.test/rtc)" "livekit-stand-in"
check "the TURN name is ended at the entry and reaches the TURN port as plain TCP" \
    "$(get https://turn.example.test/)" "turn-stand-in"

docker exec "$prefix-server" rm -f /seen/request
for path in /internal/media/notifications /internal /recording-layout/ /recording-layout/index.html; do
    answer=$(get "https://chat.example.test$path")
    case "$answer" in
        *"404"*) echo "ok    $path is closed at the entry" ;;
        *) echo "FAIL  $path answered \"$answer\""; failures=$((failures + 1)) ;;
    esac
done
check "no closed route reached the server" \
    "$(docker exec "$prefix-server" sh -c 'test -e /seen/request && echo reached || echo none')" "none"

get --header "X-Forwarded-For: 203.0.113.9" https://chat.example.test/api/server > /dev/null
sleep 0.5
forwarded=$(docker exec "$prefix-server" sh -c "tr -d '\r' < /seen/request | sed -n 's/^[Xx]-[Ff]orwarded-[Ff]or: //p'")
check "the server gets the address of the client, not the address that the client stated" "$forwarded" "127.0.0.1"

redirect=$(docker exec "$prefix-client" sh -c "printf 'GET /x HTTP/1.0\r\nHost: chat.example.test\r\n\r\n' | nc 127.0.0.1 80 | tr -d '\r' | sed -n 's/^Location: //p'")
check "port 80 redirects to HTTPS" "$redirect" "https://chat.example.test/x"

if docker logs "$prefix-entry" 2>&1 | grep -q "obtaining certificate.*acme-v02\|acme.*letsencrypt"; then
    echo "FAIL  the test contacted a public certificate authority"
    failures=$((failures + 1))
fi

if [ "$failures" -eq 0 ]; then
    echo "all checks passed"
else
    echo "$failures check(s) failed"
    exit 1
fi
