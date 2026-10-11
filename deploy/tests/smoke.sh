#!/bin/sh
# Starts the local form with new secrets and small limits, waits until every container is
# healthy, asks the health route, and removes the containers, the volumes and the network.
# The sum of the memory limits is under 3 GB. The models of the worker are not installed:
# the worker container runs and waits for jobs.
#
#   deploy/tests/smoke.sh [<project name>]
#
# JUNTO_SMOKE_KEEP=1 leaves the deployment running and prints how to remove it.
# JUNTO_LOCAL_PORT, JUNTO_LOCAL_LIVEKIT_PORT, JUNTO_LOCAL_LIVEKIT_TCP_PORT and
# JUNTO_LOCAL_LIVEKIT_UDP_PORT choose other loopback ports.
set -eu
project=${1:-junto-smoke}
deploy=$(cd "$(dirname "$0")/.." && pwd)
work=$(mktemp -d "${TMPDIR:-/tmp}/$project.XXXXXX")
port=${JUNTO_LOCAL_PORT:-8080}
cd "$deploy"

mkdir "$work/models"
(
    umask 077
    {
        ./generate-secrets.sh
        echo "JUNTO_MODELS_DIR=$work/models"
        echo "JUNTO_LOCAL_PORT=$port"
        echo "JUNTO_LOCAL_LIVEKIT_PORT=${JUNTO_LOCAL_LIVEKIT_PORT:-7880}"
        echo "JUNTO_LOCAL_LIVEKIT_TCP_PORT=${JUNTO_LOCAL_LIVEKIT_TCP_PORT:-7881}"
        echo "JUNTO_LOCAL_LIVEKIT_UDP_PORT=${JUNTO_LOCAL_LIVEKIT_UDP_PORT:-7882}"
        echo "JUNTO_POSTGRES_MEMORY=512m"
        echo "JUNTO_SERVER_MEMORY=512m"
        echo "JUNTO_REDIS_MEMORY=96m"
        echo "JUNTO_LIVEKIT_MEMORY=512m"
        echo "JUNTO_RECORDER_MEMORY=1g"
        echo "JUNTO_WORKER_MEMORY=256m"
    } > "$work/env"
)
compose() {
    docker compose -p "$project" --env-file "$work/env" -f local.yaml "$@"
}

remove() {
    code=$?
    if [ "$code" -ne 0 ]; then
        compose ps -a || true
        compose logs --tail 40 || true
    fi
    if [ "${JUNTO_SMOKE_KEEP:-}" = 1 ] && [ "$code" -eq 0 ]; then
        echo "left running. Settings: $work/env. Remove with:"
        echo "  cd $deploy && docker compose -p $project --env-file $work/env -f local.yaml down -v && rm -rf $work"
    else
        compose down -v --timeout 20 > /dev/null 2>&1 || true
        rm -rf "$work"
    fi
    exit "$code"
}
trap remove EXIT
trap 'exit 130' INT TERM

compose up -d --build --wait --wait-timeout 300

body=$(curl -fsS --max-time 10 "http://127.0.0.1:$port/api/v1/health")
case "$body" in
    *'"database":true'*) echo "ok    the health route answers 200 and reaches the database" ;;
    *) echo "FAIL  health answer: $body"; exit 1 ;;
esac

# The admission check reaches the media server within its interval of 2 seconds.
tries=0
until curl -fsS --max-time 10 "http://127.0.0.1:$port/api/v1/health" | grep -q '"media_server":true'; do
    tries=$((tries + 1))
    if [ "$tries" -ge 30 ]; then
        echo "FAIL  the server does not reach the media server"
        exit 1
    fi
    sleep 1
done
echo "ok    the server reaches the media server"

if compose logs --no-log-prefix server 2>/dev/null | grep -q "This server has no owner"; then
    echo "ok    the first start printed an owner code"
else
    echo "FAIL  the server printed no owner code"
    exit 1
fi

for service in postgres server redis livekit recorder worker; do
    state=$(compose ps --format '{{.State}}' "$service")
    if [ "$state" = running ]; then
        echo "ok    $service runs"
    else
        echo "FAIL  $service is $state"
        exit 1
    fi
done
echo "all checks passed"
