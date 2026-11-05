#!/bin/sh
# Called by linux-container.sh with the build lock held.
set -eu
root=$(CDPATH='' cd -- "$(dirname -- "$0")/../.." && pwd)
# One prefix names the image and the volumes of the runner. An instance, when set, is added to the
# names of the containers and the network, so that commands of one prefix run at the same time,
# each with its own database and media server, on the same image and volumes (ADR 0905).
prefix=${JUNTO_LINUX_PREFIX:-junto-linux}
instance=${JUNTO_LINUX_INSTANCE:-}
case "$instance" in *[!a-z0-9-]*) echo "JUNTO_LINUX_INSTANCE has a character other than a-z, 0-9 and -: $instance" >&2; exit 2 ;; esac
names=$prefix${instance:+-$instance}
image=$prefix-build:1
network=$names-tests
build=$names-build
postgres=$names-postgres
postgres_image=${JUNTO_TEST_POSTGRES_IMAGE:-postgres:17-alpine}
livekit=$names-livekit
# The settings of every form of the deployment that do not depend on the host (ADR 0904).
livekit_common=$(cat "$root/deploy/livekit-common.yaml")
livekit_image=${JUNTO_TEST_LIVEKIT_IMAGE:-livekit/livekit-server:v1.13.9@sha256:d0c04791bf63ca8dcea123571827d56ba9504bd57c8d5de60ee03b5408cc3154}
proxy=$names-control
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
# JUNTO_LINUX_CPU_SET and JUNTO_LINUX_SERVICE_CPU_SET give both sets as lists, such as 0-7,8-11;
# the CI runner selects them from the free worker locks (tools/ci/budget.py).
if [ -n "${JUNTO_LINUX_CPU_SET:-}" ]; then
    cpu_budget=$(python3 "$root/tools/linux/cpu_sets.py" --sets "$processors" "$JUNTO_LINUX_CPU_SET" \
        "${JUNTO_LINUX_SERVICE_CPU_SET:-$JUNTO_LINUX_CPU_SET}")
else
    first=${JUNTO_LINUX_CPU_FIRST:-0}
    service_first=${JUNTO_LINUX_SERVICE_CPU_FIRST:-$first}
    cpu_budget=$(python3 "$root/tools/linux/cpu_sets.py" "$processors" "$wanted" "$first" "$service_first")
fi
processors=${cpu_budget%% *}
cpu_sets=${cpu_budget#* }
build_set=${cpu_sets%% *}
service_set=${cpu_sets#* }
# JUNTO_LINUX_CARGO_JOBS sets the Cargo jobs of a command that runs programs that an earlier
# command built (tools/ci/lanes.py); the memory rule of build_jobs.py is for commands that build.
if [ -n "${JUNTO_LINUX_CARGO_JOBS:-}" ]; then
    case "$JUNTO_LINUX_CARGO_JOBS" in *[!0-9]*|0) echo "JUNTO_LINUX_CARGO_JOBS is not a count: $JUNTO_LINUX_CARGO_JOBS" >&2; exit 2 ;; esac
    jobs=$JUNTO_LINUX_CARGO_JOBS
else
    jobs=$(python3 "$root/tools/linux/build_jobs.py" "$processors" "$memory")
fi
# JUNTO_LINUX_SERVICES=none starts no PostgreSQL, LiveKit and control container, for a command
# that needs none of them.
services=${JUNTO_LINUX_SERVICES:-all}
case "$services" in all|none) ;; *) echo "JUNTO_LINUX_SERVICES is all or none: $services" >&2; exit 2 ;; esac
printf 'Build budget: CPUs %s, services %s, %s memory, %s Cargo jobs\n' "$build_set" "$service_set" "$memory" "$jobs"
if [ "$(uname -s)" = Linux ]; then
    if [ "$(id -u)" -eq 0 ]; then
        own_checkout=true
    else
        build_uid=$(id -u)
        build_gid=$(id -g)
    fi
fi
remove_all() {
    for name in $created; do docker rm -f -v "$name" >/dev/null 2>&1 || true; done
    docker network rm "$network" >/dev/null 2>&1 || true
}
client="" watcher=""
cleanup() {
    [ -z "$watcher" ] || kill "$watcher" 2>/dev/null || true
    remove_all
    # The client of the build container ends when its container is removed. A trapped signal
    # can end a wait early, so the wait is repeated while the client exists.
    if [ -n "$client" ]; then
        while kill -0 "$client" 2>/dev/null; do wait "$client" 2>/dev/null || true; done
    fi
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
# The image is built only when the key of its inputs (the files of tools/linux and the build
# arguments) differs from the label of the existing image. The classic builder enforces the same
# processor and memory bounds as the command container; BuildKit ignores these docker build
# flags. Its cache is the chain of intermediate images of this prefix.
inputs=$(python3 "$root/tools/linux/image_inputs.py" "$root/tools/linux" "BUILD_UID=$build_uid" \
    "BUILD_GID=$build_gid" "JUNTO_LINUX_IMAGE=$prefix")
existing=$(docker image inspect --format '{{index .Config.Labels "junto-linux-inputs"}}' "$image" 2>/dev/null || true)
if [ "$existing" = "$inputs" ]; then
    printf 'Image %s has the inputs %s; not built\n' "$image" "$inputs"
elif [ "${JUNTO_LINUX_IMAGE_BUILT:-}" = 1 ]; then
    echo "image $image does not have the inputs $inputs of this checkout, and JUNTO_LINUX_IMAGE_BUILT=1 forbids a build" >&2
    exit 1
else
    DOCKER_BUILDKIT=0 docker build --force-rm --label "junto-linux-inputs=$inputs" \
        --cpuset-cpus "$build_set" --memory "$memory" --memory-swap "$memory" \
        --build-arg "BUILD_UID=$build_uid" --build-arg "BUILD_GID=$build_gid" \
        --build-arg "JUNTO_LINUX_IMAGE=$prefix" -t "$image" \
        -f "$root/tools/linux/Dockerfile" "$root/tools/linux"
    # The images of this prefix that are not the new image or one of its parents are replaced
    # chains. Each is removed by its identifier with --no-prune: with the containerd image store,
    # the removal of an image with its parents also removes a parent that another image still
    # has as its parent. `docker image prune` is not used: the daemon runs one prune at a time.
    # An image that a container still uses stays.
    current=$(docker image inspect --format '{{.Id}}' "$image")
    kept=""
    while [ -n "$current" ]; do
        kept="$kept $current"
        current=$(docker image inspect --format '{{.Parent}}' "$current" 2>/dev/null || true)
    done
    replaced=""
    for candidate in $(docker image ls -a -q --no-trunc --filter "label=junto-linux-image=$prefix"); do
        case " $kept " in *" $candidate "*) ;; *) replaced="$replaced $candidate" ;; esac
    done
    if [ -n "$replaced" ]; then
        # shellcheck disable=SC2086
        docker image rm --no-prune $replaced >/dev/null || echo "some replaced images of $prefix are still in use" >&2
    fi
fi
docker volume create "$cargo_volume" >/dev/null
docker volume create "$target_volume" >/dev/null
docker network create "$network" >/dev/null
trap cleanup EXIT
trap 'exit 130' INT TERM
if [ "$services" = all ]; then
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
fi
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
# The client runs in the background, so that a signal to this script runs its cleanup at once:
# the shell runs a trap only after a command in the foreground has ended. The caller usually
# starts the lock program (tools/serial.sh), whose child this script is; when the caller ends
# that program, or when this script is killed, the watcher ends the run: it sends TERM to this
# script while the script runs, and removes the containers itself when it does not (CD-07).
# The lock is held through the inherited descriptor until the last of these processes ends.
parent=$PPID
self=$$
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
    -e JUNTO_TEST_VIRTUAL_AUDIO=1 -e TZ=UTC -e "JUNTO_LINUX_SERVICES=$services" \
    "$image" /opt/junto-linux/run-as-user.sh "$build_uid" "$build_gid" "$@" &
client=$!
(
    trap - EXIT INT TERM
    while kill -0 "$self" 2>/dev/null && [ "$(ps -o ppid= -p "$self" | tr -d ' ')" = "$parent" ]; do
        sleep 1
    done
    if kill -0 "$self" 2>/dev/null; then kill -TERM "$self"; else remove_all; fi
) &
watcher=$!
wait "$client" || status=$?
client=""
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
