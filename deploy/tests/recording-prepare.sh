#!/bin/sh
# Prepares the speech and the model files with which deploy/tests/recording.sh checks a
# transcript: the generated speech of the worker tests (workers/transcription/tests/speech,
# three files) and the model files for Whisper `small`, the development model of ADR 0612,
# with the speaker model. Only the downloads use the network: the test voices, and the model
# files from the addresses of the manifest, each kept only with its size and checksum.
#
#   deploy/tests/recording-prepare.sh <work directory> [<prefix of the image names>]
#
# The work directory gets voices/, speech/ and models/ (about 0.9 GB). Afterwards:
#
#   JUNTO_RECORDING_TEST_MODELS=<work directory>/models \
#   JUNTO_RECORDING_TEST_SPEECH=<work directory>/speech deploy/tests/recording.sh
set -eu
work=${1:?usage: recording-prepare.sh <work directory> [<prefix of the image names>]}
prefix=${2:-junto-recording-test}
worker=$(cd "$(dirname "$0")/../../workers/transcription" && pwd)

# The downloads and the generation run as the calling user, so the user of the worker can
# read the model files and cannot change them (ADR 0612).
me="$(id -u):$(id -g)"
mkdir -p "$work/voices" "$work/speech" "$work/models"
work=$(cd "$work" && pwd)

docker build -t "$prefix-speech" -f "$worker/tests/speech/Dockerfile" "$worker/tests/speech"
docker build -t "$prefix-worker" -f "$worker/docker/Dockerfile" --target worker "$worker"

docker run --rm --name "$prefix-voices" --user "$me" --memory 512m --cpus 2 \
    -v "$work/voices:/voices" -v "$worker/tests/speech:/speech:ro" \
    "$prefix-speech" sh /speech/fetch-voices.sh /voices

# The speech has a fixed seed. On another processor type its checksums can differ from the
# recorded ones; the test compares words and not checksums.
docker run --rm --name "$prefix-speech-run" --network none --user "$me" --memory 2g --cpus 4 \
    -v "$work/voices:/voices:ro" -v "$worker/tests/speech:/speech:ro" -v "$work/speech:/audio" \
    "$prefix-speech" python /speech/generate.py /speech/texts /voices /audio \
    --only en_quick,pt_quick,trio_first --check /speech/SHA256SUMS \
    || echo "the generated speech does not have the recorded checksums"

# The model files are fetched only when the directory does not hold them already.
verify() {
    docker run --rm --name "$prefix-models-verify" --network none --user "$me" --memory 512m --cpus 2 \
        -v "$work/models:/models:ro" --entrypoint junto-transcribe \
        "$prefix-worker" models verify --dir /models --whisper small
}
if ! verify > /dev/null 2>&1; then
    docker run --rm --name "$prefix-models" --user "$me" --memory 512m --cpus 2 \
        -v "$work/models:/models" --entrypoint junto-transcribe \
        "$prefix-worker" models fetch --dir /models --whisper small
fi
verify

for file in en_quick pt_quick trio_first; do
    [ -s "$work/speech/$file.wav" ] && [ -s "$work/speech/$file.json" ] \
        || { echo "the speech file $file was not generated" >&2; exit 1; }
done
echo "prepared: $work/models and $work/speech"
