#!/bin/sh
# Starts the one-host form (ADR 0003) on this machine without public DNS names: the entry
# signs its certificates itself. It checks that a request to the entry with the server name
# reaches the server, that the server reaches the API of the media server at the address of
# the host in the container network, and that the routes for the media server and the
# recorder are closed at the entry. The media server and the entry use the network of the
# host, so the ports 80, 443, 3478, 5349, 7880, 7881 and 7882 must be free. The sum of the
# memory limits is under 3 GB. Everything is removed at the end.
#
#   deploy/tests/one-host.sh [<project name>]
set -eu
project=${1:-junto-one-host}
deploy=$(cd "$(dirname "$0")/.." && pwd)
work=$(mktemp -d "${TMPDIR:-/tmp}/$project.XXXXXX")
tools_image=busybox:1.37.0@sha256:bdf57e528e45e4433820e045b29b4597825a1c9e38353532d90a01445013f82e
cd "$deploy"

compose() {
    docker compose -p "$project" --env-file "$work/env" -f one-host.yaml "$@"
}
remove() {
    code=$?
    if [ "$code" -ne 0 ]; then
        compose ps -a || true
        compose logs --tail 25 || true
    fi
    compose down -v --timeout 20 > /dev/null 2>&1 || true
    rm -rf "$work"
    exit "$code"
}
trap remove EXIT
trap 'exit 130' INT TERM

mkdir "$work/models"
(
    umask 077
    {
        ./generate-secrets.sh
        echo "JUNTO_SERVER_HOST=chat.example.test"
        echo "JUNTO_LIVEKIT_HOST=livekit.example.test"
        echo "JUNTO_TURN_HOST=turn.example.test"
        echo "JUNTO_ACME_EMAIL=admin@example.test"
        echo "JUNTO_ENTRY_CERTIFICATES=local_certs"
        echo "JUNTO_PUBLIC_IP=172.31.240.1"
        echo "JUNTO_MODELS_DIR=$work/models"
        echo "JUNTO_POSTGRES_MEMORY=512m"
        echo "JUNTO_SERVER_MEMORY=512m"
        echo "JUNTO_REDIS_MEMORY=96m"
        echo "JUNTO_LIVEKIT_MEMORY=512m"
        echo "JUNTO_RECORDER_MEMORY=768m"
        echo "JUNTO_WORKER_MEMORY=256m"
        echo "JUNTO_ENTRY_MEMORY=128m"
    } > "$work/env"
)
compose up -d --build --wait --wait-timeout 300
echo "ok    every container is healthy"

request() {
    docker run --rm --network host --add-host chat.example.test:127.0.0.1 --add-host livekit.example.test:127.0.0.1 \
        --memory 32m "$tools_image" wget -q -T 10 --no-check-certificate -O - "$@" 2>&1 || true
}
wait_for() {
    tries=0
    until request https://chat.example.test/api/v1/health | grep -q "$1"; do
        tries=$((tries + 1))
        if [ "$tries" -ge 30 ]; then
            echo "FAIL  $2: $(request https://chat.example.test/api/v1/health)"
            exit 1
        fi
        sleep 1
    done
    echo "ok    $2"
}
wait_for '"database":true' "a request to the entry with the server name reaches the server"
wait_for '"media_server":true' "the server reaches the API of the media server"

case "$(request https://chat.example.test/internal/media/notifications)" in
    *404*) echo "ok    the notification route is closed at the entry" ;;
    *) echo "FAIL  the notification route is open at the entry"; exit 1 ;;
esac
case "$(request https://livekit.example.test/)" in
    OK*) echo "ok    a request with the media server name reaches the media server" ;;
    *) echo "FAIL  the media server name: $(request https://livekit.example.test/)"; exit 1 ;;
esac
published=""
for container in $(compose ps -q); do
    published="$published$(docker port "$container")"
done
if [ -z "$published" ]; then
    echo "ok    no container publishes a port"
else
    echo "FAIL  published ports: $published"
    exit 1
fi
echo "all checks passed"
