#!/bin/sh
# Called by linux-container.sh with the build lock held.
set -eu
root=$(CDPATH='' cd -- "$(dirname -- "$0")/../.." && pwd)
# One prefix names every Docker resource of the runner.
prefix=${JUNTO_LINUX_PREFIX:-junto-linux}
image=$prefix-build:1
network=$prefix-tests
build=$prefix-build
postgres=$prefix-postgres
postgres_image=${JUNTO_TEST_POSTGRES_IMAGE:-postgres:17-alpine}
livekit=$prefix-livekit
# The settings of every form of the deployment that do not depend on the host (ADR 0904).
livekit_common=$(cat "$root/deploy/livekit-common.yaml")
livekit_image=${JUNTO_TEST_LIVEKIT_IMAGE:-livekit/livekit-server:v1.13.9@sha256:d0c04791bf63ca8dcea123571827d56ba9504bd57c8d5de60ee03b5408cc3154}
proxy=$prefix-control
cargo_volume=$prefix-cargo
target_volume=$prefix-target
created=""
socket=${JUNTO_DOCKER_SOCKET:-/var/run/docker.sock}
build_uid=1000
build_gid=1000
own_checkout=false
wanted=${JUNTO_LINUX_CPUS:-4}
memory=${JUNTO_LINUX_MEMORY:-6g}
case "$wanted" in ''|*[!0-9]*|0) echo "JUNTO_LINUX_CPUS is not a count: $wanted" >&2; exit 2 ;; esac
processors=$(docker info --format '{{.NCPU}}' 2>/dev/null || true)
case "$processors" in ''|*[!0-9]*|0) processors=$wanted ;; esac
first=${JUNTO_LINUX_CPU_FIRST:-0}
service_first=${JUNTO_LINUX_SERVICE_CPU_FIRST:-$first}
cpu_budget=$(python3 "$root/tools/linux/cpu_sets.py" "$processors" "$wanted" "$first" "$service_first")
processors=${cpu_budget%% *}
cpu_sets=${cpu_budget#* }
build_set=${cpu_sets%% *}
service_set=${cpu_sets#* }
jobs=$(python3 "$root/tools/linux/build_jobs.py" "$processors" "$memory")
printf 'Build budget: CPUs %s, services %s, %s memory, %s Cargo jobs\n' "$build_set" "$service_set" "$memory" "$jobs"
if [ "$(uname -s)" = Linux ]; then
    if [ "$(id -u)" -eq 0 ]; then
        own_checkout=true
    else
        build_uid=$(id -u)
        build_gid=$(id -g)
    fi
fi
cleanup() {
    for name in $created; do docker rm -f -v "$name" >/dev/null 2>&1 || true; done
    docker network rm "$network" >/dev/null 2>&1 || true
}
# Existing names indicate another run or interrupted cleanup; never replace them.
for name in "$build" "$postgres" "$livekit" "$proxy"; do
    if docker container inspect "$name" >/dev/null 2>&1; then
        echo "container $name already exists" >&2
        exit 1
    fi
done
if docker network inspect "$network" >/dev/null 2>&1; then
    echo "network $network already exists; inspect and remove the interrupted run before retrying" >&2
    exit 1
fi
# JUNTO_LINUX_BUILDER=limited builds the image with the builder of Docker without BuildKit:
# it runs each step in a container with the processor and memory limits of the build
# container below, and it keeps no BuildKit cache; its cache is the layers of the image. The
# label names the image of this prefix, so that an image that a new build replaces is
# removed without touching other images.
if [ "${JUNTO_LINUX_BUILDER:-}" = limited ]; then
    build_memory=$memory
    DOCKER_BUILDKIT=0 docker build --force-rm --label "junto-linux-image=$prefix" \
        --cpuset-cpus "$build_set" --memory "$build_memory" --memory-swap "$build_memory" \
        --build-arg "BUILD_UID=$build_uid" --build-arg "BUILD_GID=$build_gid" -t "$image" \
        -f "$root/tools/linux/Dockerfile" "$root/tools/linux"
    docker image prune -f --filter "label=junto-linux-image=$prefix" >/dev/null
else
    docker build --build-arg "BUILD_UID=$build_uid" --build-arg "BUILD_GID=$build_gid" -t "$image" -f "$root/tools/linux/Dockerfile" "$root/tools/linux"
fi
docker volume create "$cargo_volume" >/dev/null
docker volume create "$target_volume" >/dev/null
docker network create "$network" >/dev/null
trap cleanup EXIT
trap 'exit 130' INT TERM
created="$postgres"
docker run -d --name "$postgres" --network "$network" --network-alias postgres \
    --cpuset-cpus "$service_set" --cpus "${JUNTO_LINUX_POSTGRES_CPUS:-0.75}" --memory 1536m --memory-swap 1536m \
    -e POSTGRES_HOST_AUTH_METHOD=trust \
    "$postgres_image" \
    -c max_connections=400 -c fsync=off -c shared_preload_libraries=pg_stat_statements \
    -c pg_stat_statements.max=100000 >/dev/null
created="$livekit $created"
docker run -d --name "$livekit" --network "$network" --network-alias livekit \
    --cpuset-cpus "$service_set" --cpus 1 --memory 768m --memory-swap 768m \
    -e "LIVEKIT_CONFIG=port: 7880
keys:
  devkey: secret
rtc:
  tcp_port: 7881
  udp_port: 7882
  use_external_ip: false
webhook:
  api_key: devkey
  urls:
    - http://$build:17890/internal/media/notifications
$livekit_common
" "$livekit_image" \
    --dev --bind 0.0.0.0 >/dev/null
created="$proxy $created"
docker run -d --user 0 --name "$proxy" --network "$network" --network-alias control \
    --cpuset-cpus "$service_set" --cpus 0.25 --memory 128m --memory-swap 128m \
    --mount "type=bind,src=$socket,dst=/var/run/docker.sock" \
    -e JUNTO_TEST_LIVEKIT_CONTAINER="$livekit" \
    "$image" python3 /opt/junto-linux/docker-control.py >/dev/null
created="$build $created"
# Docker creates a missing mount point as its own user, which is root on a Linux host.
mkdir -p "$root/spikes/test-participant/target"
# The build gets at most JUNTO_LINUX_CPUS processors (default 4) as a set and not as a
# quota, and JUNTO_LINUX_MEMORY of memory (default 6g). A quota stops every thread of the
# container for the rest of each 100 ms in which it is used up, and a test that measures
# sound in real time then measures those stops. Cargo jobs also fit the memory budget:
# 2 GiB for the session and other work, and 3 GiB per compiler or linker job (ADR 0905).
# The other three containers have quotas:
# PostgreSQL defaults to 0.75 processor (JUNTO_LINUX_POSTGRES_CPUS replaces it), LiveKit gets
# 1 and control 0.25; together they have 2.4 GiB of memory. No container may use swap.
# Test service ports are not published; no host display, sound or home is mounted.
if "$own_checkout"; then
    set -- /src /target /usr/local/cargo/registry -- /opt/junto-linux/session.sh "$@"
else
    set -- /target /usr/local/cargo/registry -- /opt/junto-linux/session.sh "$@"
fi
status=0
run_build() {
    if [ "${JUNTO_LINUX_CAMERA:-}" = 1 ]; then
        python3 "$root/tools/linux/camera.py" "$@"
    else
        "$@"
    fi
}
run_build docker run --name "$build" --user 0 --network "$network" --memory "$memory" --memory-swap "$memory" --cpuset-cpus "$build_set" --shm-size 512m \
    --mount "type=bind,src=$root,dst=/src" \
    --mount "type=volume,src=$cargo_volume,dst=/usr/local/cargo/registry" \
    --mount "type=volume,src=$target_volume,dst=/target" \
    --mount "type=volume,src=$target_volume,dst=/src/spikes/test-participant/target" \
    -e "CARGO_BUILD_JOBS=$jobs" -e CARGO_TARGET_DIR=/target -e CARGO_INCREMENTAL -e CARGO_TERM_COLOR \
    -e JUNTO_UPDATE_SOURCE -e SOURCE_DATE_EPOCH -e JUNTO_CI_TOOLS \
    -e JUNTO_TEST_DATABASE_URL=postgres://postgres@postgres/postgres \
    -e JUNTO_TEST_LIVEKIT_API_URL=http://livekit:7880 \
    -e JUNTO_TEST_LIVEKIT_URL=ws://livekit:7880 \
    -e JUNTO_TEST_LIVEKIT_API_KEY=devkey -e JUNTO_TEST_LIVEKIT_API_SECRET=secret \
    -e JUNTO_TEST_LIVEKIT_CONTAINER="$livekit" \
    -e JUNTO_TEST_WEBHOOK_ADDRESS=0.0.0.0:17890 \
    -e JUNTO_TEST_VIRTUAL_AUDIO=1 -e TZ=UTC \
    "$image" /opt/junto-linux/run-as-user.sh "$build_uid" "$build_gid" "$@" || status=$?
if [ "$status" -ne 0 ]; then
    docker logs "$postgres" 2>&1 | grep -E \
        'PANIC|FATAL|No space left|server process.*terminated|database system.*interrupted|recovery in progress' \
        >&2 || true
fi
if [ -n "${JUNTO_LINUX_ARTIFACTS:-}" ]; then
    mkdir -p "$JUNTO_LINUX_ARTIFACTS"
    if ! docker cp "$build:/tmp/junto-linux-artifacts/." "$JUNTO_LINUX_ARTIFACTS/"; then
        echo "could not copy Linux artifacts" >&2
        if [ "$status" -eq 0 ]; then status=1; fi
    fi
fi
exit "$status"
