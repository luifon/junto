#!/bin/sh
# Prepares a machine for the model tests: builds the images, downloads the test voices and the
# models, and generates the test speech. Only the downloads use the network.
#
# Usage: prepare.sh <work directory> [cpu] [gpu]
# The work directory gets voices/, audio/, models/ and out/. Run from workers/transcription.
set -eu
work="${1:?usage: prepare.sh <work directory> [cpu] [gpu]}"
shift
variants="${*:-cpu gpu}"
prefix="${JUNTO_TEST_PREFIX:-junto-worker-test}"
limits="${JUNTO_TEST_LIMITS:---cpus 6 --memory 10g}"
here="$(cd "$(dirname "$0")/../.." && pwd)"

# The downloads and the speech generation run as the calling user, so the worker's user can read
# the results and cannot change them. Only the output directory of the tests is open to it.
me="$(id -u):$(id -g)"
mkdir -p "$work/voices" "$work/audio" "$work/models" "$work/out"
chmod 0777 "$work/out"

docker build -t "$prefix-speech" -f "$here/tests/speech/Dockerfile" "$here/tests/speech"
for variant in $variants; do
  docker build -t "$prefix-$variant" -f "$here/docker/Dockerfile" --target test --build-arg "VARIANT=$variant" "$here"
done
first="${variants%% *}"

docker run --rm --name "$prefix-voices" --user "$me" $limits \
  -v "$work/voices:/voices" -v "$here/tests/speech:/speech:ro" \
  "$prefix-speech" sh /speech/fetch-voices.sh /voices

# The speech is generated with a fixed seed. Its checksums are compared with the recorded ones;
# on another processor type they can differ, and then the results can differ from the README.
check=""
[ -f "$here/tests/speech/SHA256SUMS" ] && check="--check /speech/SHA256SUMS"
docker run --rm --name "$prefix-speech-run" --network none --user "$me" $limits \
  -v "$work/voices:/voices:ro" -v "$here/tests/speech:/speech:ro" -v "$work/audio:/audio" \
  "$prefix-speech" python /speech/generate.py /speech/texts /voices /audio $check \
  || echo "the generated speech does not have the recorded checksums"

docker run --rm --name "$prefix-models" --user "$me" $limits \
  -v "$work/models:/models" --entrypoint junto-transcribe \
  "$prefix-$first" models fetch --dir /models ${JUNTO_TEST_WHISPER:+--whisper "$JUNTO_TEST_WHISPER"}
