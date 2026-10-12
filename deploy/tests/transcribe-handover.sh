#!/bin/sh
# Tests the hand-over of a transcription job between the server container and the worker
# container (ADR 0016) with two users and a stand-in worker, inside the server image.
#
#   deploy/tests/transcribe-handover.sh <server image>
set -eu
image=${1:?usage: transcribe-handover.sh <server image>}
deploy=$(cd "$(dirname "$0")/.." && pwd)
exec docker run --rm --no-healthcheck --network none --user 0:0 --read-only --tmpfs /work:exec \
    --cap-drop ALL --cap-add SETUID --cap-add SETGID --cap-add CHOWN --cap-add FOWNER \
    --cap-add DAC_OVERRIDE --cap-add KILL --cap-add SETPCAP \
    --memory 256m --cpus 2 --pids-limit 256 \
    -v "$deploy:/deploy:ro" \
    --entrypoint /bin/bash "$image" /deploy/tests/transcribe-handover-inside.sh
