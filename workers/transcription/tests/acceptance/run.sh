#!/bin/sh
# Runs the tests in the test image without network access.
#
# Usage: run.sh <work directory> <cpu|gpu> [pytest arguments]
# Environment: JUNTO_TEST_PROFILE (quick or full), JUNTO_TEST_WHISPER (a Whisper model of the manifest).
set -eu
work="${1:?usage: run.sh <work directory> <cpu|gpu> [pytest arguments]}"
variant="${2:?usage: run.sh <work directory> <cpu|gpu> [pytest arguments]}"
shift 2
prefix="${JUNTO_TEST_PREFIX:-junto-worker-test}"
limits="${JUNTO_TEST_LIMITS:---cpus 6 --memory 10g}"
gpu=""
if [ "$variant" = gpu ]; then
  gpu="--gpus all"
  # Some systems give the GPU device nodes to a group instead of to everyone.
  case "$(stat -c %a /dev/nvidiactl)" in *6) ;; *) gpu="$gpu --group-add $(stat -c %g /dev/nvidiactl)" ;; esac
fi

exec docker run --rm --name "$prefix-run-$variant" --network none $gpu $limits \
  --read-only --tmpfs /tmp --cap-drop ALL --security-opt no-new-privileges:true \
  -v "$work/models:/models:ro" -v "$work/audio:/audio:ro" -v "$work/out:/out" \
  -e JUNTO_TRANSCRIBE_TEST_AUDIO=/audio \
  -e JUNTO_TRANSCRIBE_TEST_OUT=/out \
  -e JUNTO_TRANSCRIBE_TEST_REQUIRE=1 \
  -e "JUNTO_TRANSCRIBE_TEST_PROFILE=${JUNTO_TEST_PROFILE:-full}" \
  -e "JUNTO_TRANSCRIBE_WHISPER_MODEL=${JUNTO_TEST_WHISPER:-large-v3-turbo}" \
  -e "JUNTO_TRANSCRIBE_DEVICE=$([ "$variant" = gpu ] && echo cuda || echo cpu)" \
  "$prefix-$variant" "$@"
