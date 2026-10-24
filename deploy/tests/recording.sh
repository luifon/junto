#!/bin/sh
# Records a call through the local form with the real parts (ADR 0020): the server, the media
# server, the recorder with the built layout page that the server serves, the files on the
# shared volume, the transcription worker, and the downloads. Three test participants publish
# generated sound and pictures. Then the failures: everybody leaves the call, the recorder is
# killed during a recording, the server is restarted during a recording, the recorder is
# stopped during a recording and stays stopped, a recording is removed while its transcript
# runs, a backup is made while a recording runs and while a transcript runs and each is
# restored, and the storage guard refuses a start. recording_driver.py holds the steps and
# the checks.
#
#   (cd spikes/test-participant && cargo build --release)
#   deploy/tests/recording.sh [<project name>]
#
# Needs Docker with Compose, curl, python3 and the test participant. Needs 13 GiB of free
# disk space for Docker (ADR 0605) and four free CPU cores. The sum of the memory limits is
# under 4 GB. Everything is removed at the end.
#
# JUNTO_TEST_PARTICIPANT       the program of spikes/test-participant, when it is elsewhere.
# JUNTO_RECORDING_TEST_MODELS  a directory with the model files of the worker for Whisper
# JUNTO_RECORDING_TEST_SPEECH  `small`, and a directory with generated speech
#                              (deploy/tests/recording-prepare.sh makes both). With them the
#                              participants speak and the transcript is checked. Without
#                              them the worker runs as far as its check of the model files,
#                              and the removal and the backup during a transcript are not run.
# JUNTO_RECORDING_TEST_KEEP=1  leaves the deployment running after a failure and prints how
#                              to remove it.
# JUNTO_RECORDING_TEST_FILES   a directory that gets the video and one audio file of the
#                              first recording, as downloaded, for a look at them.
# JUNTO_LOCAL_PORT, JUNTO_LOCAL_LIVEKIT_PORT, JUNTO_LOCAL_LIVEKIT_TCP_PORT and
# JUNTO_LOCAL_LIVEKIT_UDP_PORT choose other loopback ports.
set -eu
project=${1:-junto-recording-test}
deploy=$(cd "$(dirname "$0")/.." && pwd)
participant=${JUNTO_TEST_PARTICIPANT:-$deploy/../spikes/test-participant/target/release/test-participant}
models=${JUNTO_RECORDING_TEST_MODELS:-}
speech=${JUNTO_RECORDING_TEST_SPEECH:-}
port=${JUNTO_LOCAL_PORT:-8080}

if [ ! -x "$participant" ]; then
    echo "the test participant is not built: (cd spikes/test-participant && cargo build --release)" >&2
    exit 2
fi
if { [ -n "$models" ] && [ -z "$speech" ]; } || { [ -z "$models" ] && [ -n "$speech" ]; }; then
    echo "JUNTO_RECORDING_TEST_MODELS and JUNTO_RECORDING_TEST_SPEECH are given together" >&2
    exit 2
fi
for file in en_quick pt_quick trio_first; do
    if [ -n "$speech" ] && { [ ! -f "$speech/$file.wav" ] || [ ! -f "$speech/$file.json" ]; }; then
        echo "$speech has no $file.wav with $file.json (deploy/tests/recording-prepare.sh)" >&2
        exit 2
    fi
done

work=$(mktemp -d "${TMPDIR:-/tmp}/$project.XXXXXX")
cd "$deploy"
mkdir "$work/models"
(
    umask 077
    {
        ./generate-secrets.sh
        echo "JUNTO_MODELS_DIR=${models:-$work/models}"
        echo "JUNTO_TRANSCRIBE_WHISPER_MODEL=small"
        # With the batch size of the model the worker needs 2.1 GB for this model.
        echo "JUNTO_TRANSCRIBE_BATCH_SIZE=1"
        echo "JUNTO_LOCAL_PORT=$port"
        echo "JUNTO_LOCAL_LIVEKIT_PORT=${JUNTO_LOCAL_LIVEKIT_PORT:-7880}"
        echo "JUNTO_LOCAL_LIVEKIT_TCP_PORT=${JUNTO_LOCAL_LIVEKIT_TCP_PORT:-7881}"
        echo "JUNTO_LOCAL_LIVEKIT_UDP_PORT=${JUNTO_LOCAL_LIVEKIT_UDP_PORT:-7882}"
        echo "JUNTO_POSTGRES_MEMORY=256m"
        echo "JUNTO_SERVER_MEMORY=384m"
        echo "JUNTO_REDIS_MEMORY=64m"
        echo "JUNTO_LIVEKIT_MEMORY=320m"
        echo "JUNTO_RECORDER_MEMORY=1280m"
        echo "JUNTO_WORKER_MEMORY=1408m"
        # A restore names the images of the server and of init (deploy/restore-images.yaml).
        echo "JUNTO_SERVER_IMAGE=$project-server"
        echo "JUNTO_INIT_IMAGE=$project-init"
    } > "$work/env"
)

forms="local.yaml"
# Prints the Compose files of the deployment, each after the option $1.
with_forms() {
    for form in $forms; do
        printf '%s %s ' "$1" "$form"
    done
}
compose() {
    # shellcheck disable=SC2046
    docker compose -p "$project" --env-file "$work/env" $(with_forms -f) "$@"
}
driver() {
    # shellcheck disable=SC2046
    python3 -B "$deploy/tests/recording_driver.py" "$1" --project "$project" --env-file "$work/env" \
        $(with_forms --compose-file) --origin "http://127.0.0.1:$port" --participant "$participant" --work "$work" \
        ${speech:+--speech "$speech" --models} \
        ${JUNTO_RECORDING_TEST_FILES:+--keep-files "$JUNTO_RECORDING_TEST_FILES"}
}

remove() {
    code=$?
    if [ "$code" -ne 0 ]; then
        compose ps -a || true
        # The server prints its one-time owner code on a line of its own; that line is left out.
        compose logs --tail 40 server recorder worker 2>&1 | grep -v -E '\|  +[A-Za-z0-9-]+ *$' || true
    fi
    if [ "${JUNTO_RECORDING_TEST_KEEP:-}" = 1 ] && [ "$code" -ne 0 ]; then
        echo "left running. Settings: $work/env. Remove with:"
        echo "  cd $deploy && docker compose -p $project --env-file $work/env $(with_forms -f)down -v && rm -rf $work"
    elif compose down -v --timeout 20 > /dev/null 2>&1; then
        docker image rm "$project-server" "$project-init" "$project-worker" > /dev/null 2>&1 || true
        rm -rf "$work"
    else
        echo "FAIL  the deployment could not be removed. Settings: $work/env. Remove with:"
        echo "  cd $deploy && docker compose -p $project --env-file $work/env $(with_forms -f)down -v && rm -rf $work"
        [ "$code" -ne 0 ] || code=1
    fi
    exit "$code"
}
trap remove EXIT
trap 'exit 130' INT TERM

compose up -d --build --wait --wait-timeout 600
driver main

# The storage guard: the same deployment, new, with a volume of the recordings that has
# less free space than the reserve.
compose down -v --timeout 20
forms="local.yaml tests/recording-low-storage.yaml"
compose up -d --wait --wait-timeout 600
driver low-storage
echo "all checks passed"
