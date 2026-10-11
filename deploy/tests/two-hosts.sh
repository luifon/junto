#!/bin/sh
# Starts the two files of the two-host form (ADR 0003) on one machine, as two Compose
# projects with separate networks, and checks the paths through the private link:
# - the two ends of the link complete a handshake;
# - a request to the entry with the server name reaches the server on the "data host";
# - the server reaches the API of the media server on the "public host";
# - the recorder reaches Redis.
# The entry signs its certificates itself. The media server and the entry use the network
# of the host, so the ports 80, 443, 3478, 5349, 7880, 7881 and 7882 must be free. The sum
# of the memory limits is under 3 GB. Everything is removed at the end.
#
#   deploy/tests/two-hosts.sh [<name prefix of the two projects>]
set -eu
prefix=${1:-junto-two-hosts}
deploy=$(cd "$(dirname "$0")/.." && pwd)
work=$(mktemp -d "${TMPDIR:-/tmp}/$prefix.XXXXXX")
tools_image=busybox:1.37.0@sha256:bdf57e528e45e4433820e045b29b4597825a1c9e38353532d90a01445013f82e
cd "$deploy"

public() {
    docker compose -p "$prefix-public" --env-file "$work/public.env" -f two-hosts-public.yaml "$@"
}
data() {
    docker compose -p "$prefix-data" --env-file "$work/data.env" -f two-hosts-data.yaml "$@"
}
remove() {
    code=$?
    if [ "$code" -ne 0 ]; then
        public ps -a || true
        data ps -a || true
        public logs --tail 25 || true
        data logs --tail 25 || true
    fi
    data down -v --timeout 20 > /dev/null 2>&1 || true
    public down -v --timeout 20 > /dev/null 2>&1 || true
    rm -rf "$work"
    exit "$code"
}
trap remove EXIT
trap 'exit 130' INT TERM

mkdir "$work/models"
(
    umask 077
    ./generate-secrets.sh > "$work/secrets"
    {
        grep -v JUNTO_POSTGRES_PASSWORD "$work/secrets"
        echo "JUNTO_SERVER_HOST=chat.example.test"
        echo "JUNTO_LIVEKIT_HOST=livekit.example.test"
        echo "JUNTO_TURN_HOST=turn.example.test"
        echo "JUNTO_ACME_EMAIL=admin@example.test"
        echo "JUNTO_ENTRY_CERTIFICATES=local_certs"
        echo "JUNTO_PUBLIC_IP=172.31.240.1"
        echo "JUNTO_REDIS_MEMORY=96m"
        echo "JUNTO_LIVEKIT_MEMORY=512m"
        echo "JUNTO_ENTRY_MEMORY=128m"
        echo "JUNTO_LINK_MEMORY=64m"
    } > "$work/public.env"
    {
        cat "$work/secrets"
        echo "JUNTO_SERVER_HOST=chat.example.test"
        echo "JUNTO_LIVEKIT_HOST=livekit.example.test"
        echo "JUNTO_MODELS_DIR=$work/models"
        # The "public host" is reached at the address of the machine in its own network.
        echo "JUNTO_PUBLIC_IP=172.31.240.1"
        echo "JUNTO_BACKEND_SUBNET=172.31.241.0/24"
        echo "JUNTO_BACKEND_GATEWAY=172.31.241.1"
        echo "JUNTO_BACKEND_SERVER=172.31.241.10"
        echo "JUNTO_BACKEND_REDIS=172.31.241.11"
        echo "JUNTO_BACKEND_LINK=172.31.241.12"
        echo "JUNTO_POSTGRES_MEMORY=512m"
        echo "JUNTO_SERVER_MEMORY=512m"
        echo "JUNTO_RECORDER_MEMORY=768m"
        echo "JUNTO_WORKER_MEMORY=128m"
        echo "JUNTO_LINK_MEMORY=64m"
    } > "$work/data.env"
)
# The order of docs/install.md: each host creates its key before it has the key of the other.
public build link
data build
public_key=$(public run --rm --no-deps -T link genkey)
data_key=$(data run --rm --no-deps -T link genkey)
(
    umask 077
    echo "JUNTO_LINK_DATA_PUBLIC_KEY=$data_key" >> "$work/public.env"
    echo "JUNTO_LINK_PUBLIC_PUBLIC_KEY=$public_key" >> "$work/data.env"
)

# The link of the public host is healthy only after the data host connected.
public up -d
data up -d --wait --wait-timeout 300
public up -d --wait --wait-timeout 120
echo "ok    every container of both projects is healthy, the two ends of the link included"

request() {
    docker run --rm --network host --add-host chat.example.test:127.0.0.1 --memory 32m "$tools_image" \
        wget -q -T 10 --no-check-certificate -O - "$@" 2>&1 || true
}
tries=0
until request https://chat.example.test/api/v1/health | grep -q '"database":true'; do
    tries=$((tries + 1))
    if [ "$tries" -ge 30 ]; then
        echo "FAIL  the entry does not reach the server through the link: $(request https://chat.example.test/api/v1/health)"
        exit 1
    fi
    sleep 1
done
echo "ok    a request to the entry with the server name reaches the server through the link"

tries=0
until request https://chat.example.test/api/v1/health | grep -q '"media_server":true'; do
    tries=$((tries + 1))
    if [ "$tries" -ge 30 ]; then
        echo "FAIL  the server does not reach the API of the media server through the link"
        exit 1
    fi
    sleep 1
done
echo "ok    the server reaches the API of the media server through the link"

case "$(request https://chat.example.test/internal/media/notifications)" in
    *404*) echo "ok    the notification route is closed at the entry" ;;
    *) echo "FAIL  the notification route is open at the entry"; exit 1 ;;
esac

if data exec -T recorder bash -c 'exec 3<>/dev/tcp/'"172.31.241.12"'/6379 && printf "PING\r\n" >&3 && IFS= read -r -t 5 answer <&3 && case "$answer" in *NOAUTH*) exit 0 ;; *) exit 1 ;; esac' ; then
    echo "ok    the recorder container reaches Redis through the link, and Redis asks for the password"
else
    echo "FAIL  the recorder container does not reach Redis through the link"
    exit 1
fi
echo "all checks passed"
