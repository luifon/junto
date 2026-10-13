#!/bin/sh
# Called by linux-container.sh with the build lock held.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
# One prefix names every Docker resource of the runner.
prefix=${JUNTO_LINUX_PREFIX:-junto-linux}
image=$prefix-build:1
network=$prefix-tests
build=$prefix-build
postgres=$prefix-postgres
postgres_image=${JUNTO_TEST_POSTGRES_IMAGE:-postgres:17-alpine}
livekit=$prefix-livekit
livekit_image=${JUNTO_TEST_LIVEKIT_IMAGE:-livekit/livekit-server:v1.13.9@sha256:d0c04791bf63ca8dcea123571827d56ba9504bd57c8d5de60ee03b5408cc3154}
proxy=$prefix-control
cargo_volume=$prefix-cargo
target_volume=$prefix-target
created=""
socket=${JUNTO_DOCKER_SOCKET:-/var/run/docker.sock}
build_uid=1000
build_gid=1000
if [ "$(uname -s)" = Linux ]; then
    build_uid=$(id -u)
    build_gid=$(id -g)
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
docker build --build-arg "BUILD_UID=$build_uid" --build-arg "BUILD_GID=$build_gid" -t "$image" -f "$root/tools/linux/Dockerfile" "$root/tools/linux"
docker volume create "$cargo_volume" >/dev/null
docker volume create "$target_volume" >/dev/null
docker run --rm --name "$prefix-cache-init" --user 0 \
    --mount "type=volume,src=$cargo_volume,dst=/usr/local/cargo/registry" \
    --mount "type=volume,src=$target_volume,dst=/target" \
    -e "JUNTO_BUILD_UID=$build_uid" -e "JUNTO_BUILD_GID=$build_gid" \
    "$image" sh -c 'for directory in /target /usr/local/cargo/registry; do
        if [ "$(stat -c %u:%g "$directory")" != "$JUNTO_BUILD_UID:$JUNTO_BUILD_GID" ]; then
            chown -R "$JUNTO_BUILD_UID:$JUNTO_BUILD_GID" "$directory"
        fi
    done'
docker network create "$network" >/dev/null
trap cleanup EXIT
trap 'exit 130' INT TERM
created="$postgres"
docker run -d --name "$postgres" --network "$network" --network-alias postgres \
    -e POSTGRES_HOST_AUTH_METHOD=trust \
    "$postgres_image" \
    -c max_connections=400 -c fsync=off -c shared_preload_libraries=pg_stat_statements \
    -c pg_stat_statements.max=100000 >/dev/null
created="$livekit $created"
docker run -d --name "$livekit" --network "$network" --network-alias livekit \
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
" "$livekit_image" \
    --dev --bind 0.0.0.0 >/dev/null
created="$proxy $created"
docker run -d --user 0 --name "$proxy" --network "$network" --network-alias control \
    --mount "type=bind,src=$socket,dst=/var/run/docker.sock" \
    -e JUNTO_TEST_LIVEKIT_CONTAINER="$livekit" \
    "$image" python3 /opt/junto-linux/docker-control.py >/dev/null
created="$build $created"
# Docker creates a missing mount point as its own user, which is root on a Linux host.
mkdir -p "$root/spikes/test-participant/target"
# Test service ports are not published; no host display, sound or home is mounted.
status=0
docker run --name "$build" --network "$network" --memory 6g --cpus 4 --shm-size 512m \
    --mount "type=bind,src=$root,dst=/src" \
    --mount "type=volume,src=$cargo_volume,dst=/usr/local/cargo/registry" \
    --mount "type=volume,src=$target_volume,dst=/target" \
    --mount "type=volume,src=$target_volume,dst=/src/spikes/test-participant/target" \
    -e CARGO_BUILD_JOBS=4 -e CARGO_TARGET_DIR=/target -e CARGO_INCREMENTAL -e CARGO_TERM_COLOR \
    -e JUNTO_TEST_DATABASE_URL=postgres://postgres@postgres/postgres \
    -e JUNTO_TEST_LIVEKIT_API_URL=http://livekit:7880 \
    -e JUNTO_TEST_LIVEKIT_URL=ws://livekit:7880 \
    -e JUNTO_TEST_LIVEKIT_API_KEY=devkey -e JUNTO_TEST_LIVEKIT_API_SECRET=secret \
    -e JUNTO_TEST_LIVEKIT_CONTAINER="$livekit" \
    -e JUNTO_TEST_WEBHOOK_ADDRESS=0.0.0.0:17890 \
    -e JUNTO_TEST_VIRTUAL_AUDIO=1 -e TZ=UTC \
    "$image" /opt/junto-linux/session.sh "$@" || status=$?
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
