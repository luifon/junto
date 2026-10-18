#!/bin/sh
# Runs a command with a temporary LiveKit server in Docker, in development mode, and removes
# it afterwards. A failed run prints the container state and timestamped logs on standard
# error before removal, so the output of the test keeps the media-server evidence too.
# The command receives JUNTO_TEST_LIVEKIT_API_URL, JUNTO_TEST_LIVEKIT_URL,
# JUNTO_TEST_LIVEKIT_API_KEY, JUNTO_TEST_LIVEKIT_API_SECRET, JUNTO_TEST_WEBHOOK_ADDRESS and
# JUNTO_TEST_LIVEKIT_CONTAINER, the name of the container: a test restarts it to test a
# reconnect.
#
# The media ports are fixed, because the server announces them to its clients. Set
# JUNTO_TEST_LIVEKIT_PORT (signalling and API, TCP), JUNTO_TEST_LIVEKIT_TCP_PORT and
# JUNTO_TEST_LIVEKIT_UDP_PORT when the defaults are taken. The server sends its notifications
# to JUNTO_TEST_WEBHOOK_ADDRESS on the machine that runs the tests.
#
# The container is named by JUNTO_TEST_LIVEKIT_CONTAINER (default junto-dev-livekit). Two
# checkouts on one machine run their tests at the same time when each sets another name and
# other ports. JUNTO_TEST_LIVEKIT_CPUS and JUNTO_TEST_LIVEKIT_MEMORY limit the container
# (default 2 processors and 1g, without swap).
set -eu

if [ "$#" -eq 0 ]; then
    echo "usage: $0 <command> [arguments]" >&2
    exit 2
fi

name="${JUNTO_TEST_LIVEKIT_CONTAINER:-junto-dev-livekit}"
image="${JUNTO_TEST_LIVEKIT_IMAGE:-livekit/livekit-server:v1.13.9}"
port="${JUNTO_TEST_LIVEKIT_PORT:-17880}"
tcp_port="${JUNTO_TEST_LIVEKIT_TCP_PORT:-17881}"
udp_port="${JUNTO_TEST_LIVEKIT_UDP_PORT:-17882}"
webhook_address="${JUNTO_TEST_WEBHOOK_ADDRESS:-127.0.0.1:17890}"
webhook_port="${webhook_address##*:}"
key="${JUNTO_TEST_LIVEKIT_API_KEY:-devkey}"
secret="${JUNTO_TEST_LIVEKIT_API_SECRET:-secret}"

remove() {
    docker rm -f -v "$name" >/dev/null 2>&1 || true
}

finish() {
    test_status=$?
    if [ "$test_status" -ne 0 ]; then
        printf '\nLiveKit container %s after failure (exit %s):\n' "$name" "$test_status" >&2
        docker inspect --format '{{json .State}}' "$name" >&2 || true
        docker logs --timestamps "$name" >&2 || true
    fi
    remove
    exit "$test_status"
}

remove
trap finish EXIT
trap 'exit 130' INT TERM

config="port: 7880
keys:
  ${key}: ${secret}
rtc:
  tcp_port: ${tcp_port}
  udp_port: ${udp_port}
  use_external_ip: false
  node_ip: 127.0.0.1
webhook:
  api_key: ${key}
  urls:
    - http://host.docker.internal:${webhook_port}/internal/media/notifications
"
# The setting of the camera layers in the configuration of the deployment
# (deploy/tests/check_rendered.py compares them). The tests of the recording layout set
# JUNTO_TEST_LIVEKIT_DEPLOYMENT_LAYERS=1; the other tests use the defaults of LiveKit.
deployment_layers="video:
  stream_tracker_manager:
    video:
      packet_tracker:
        1:
          samples_required: 1
          cycles_required: 4
          cycle_duration: 500ms
        2:
          samples_required: 1
          cycles_required: 4
          cycle_duration: 500ms
"
if [ "${JUNTO_TEST_LIVEKIT_DEPLOYMENT_LAYERS:-}" = 1 ]; then
    config="${config}${deployment_layers}"
fi

docker run -d --name "$name" \
    --cpus "${JUNTO_TEST_LIVEKIT_CPUS:-2}" --memory "${JUNTO_TEST_LIVEKIT_MEMORY:-1g}" \
    --memory-swap "${JUNTO_TEST_LIVEKIT_MEMORY:-1g}" \
    --add-host=host.docker.internal:host-gateway \
    -e LIVEKIT_CONFIG="$config" \
    -p "127.0.0.1:${port}:7880" \
    -p "127.0.0.1:${tcp_port}:${tcp_port}" \
    -p "127.0.0.1:${udp_port}:${udp_port}/udp" \
    "$image" --dev --bind 0.0.0.0 >/dev/null

attempts=0
until curl -fsS -o /dev/null "http://127.0.0.1:${port}/"; do
    attempts=$((attempts + 1))
    if [ "$attempts" -ge 120 ]; then
        echo "LiveKit did not become ready" >&2
        exit 1
    fi
    sleep 0.5
done

export JUNTO_TEST_LIVEKIT_API_URL="http://127.0.0.1:${port}"
export JUNTO_TEST_LIVEKIT_URL="ws://127.0.0.1:${port}"
export JUNTO_TEST_LIVEKIT_API_KEY="$key"
export JUNTO_TEST_LIVEKIT_API_SECRET="$secret"
export JUNTO_TEST_WEBHOOK_ADDRESS="$webhook_address"
export JUNTO_TEST_LIVEKIT_CONTAINER="$name"
"$@"
