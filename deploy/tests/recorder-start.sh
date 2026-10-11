#!/bin/sh
# Tests the start script of the recorder container (deploy/images/recorder/start.sh) in the
# server image, which has bash: partial audio files are copied to rescued/<job>/ before the
# start script of the image runs, and a copy that fails stops the start, so that the start
# script of the image does not delete the only copy.
#
#   deploy/tests/recorder-start.sh <server image>
set -eu
image=${1:?usage: recorder-start.sh <server image>}
deploy=$(cd "$(dirname "$0")/.." && pwd)
exec docker run --rm --no-healthcheck --network none --read-only --tmpfs /work:exec,uid=1001,gid=1001 \
    --cap-drop ALL --memory 128m --cpus 1 --pids-limit 64 \
    -v "$deploy:/deploy:ro" --entrypoint /bin/bash "$image" -c '
set -u
failures=0
check() {
    if eval "$2"; then echo "ok    $1"; else echo "FAIL  $1"; failures=$((failures + 1)); fi
}
# The stand-in for the start script of the image deletes the temporary directory, as the
# real one does, and leaves a mark.
printf "#!/bin/bash\nrm -rf \"\$JUNTO_RECORDER_WORK\"/*\necho started > /work/image-started\n" > /work/image-start
chmod 755 /work/image-start
prepare() {
    rm -rf /work/tmp /work/recordings /work/image-started
    mkdir -p /work/tmp/EG_one /work/tmp/EG_two/chrome /work/recordings
    echo partial-one > /work/tmp/EG_one/segment-a.ogg
    echo partial-two > /work/tmp/EG_two/segment-b.ogg
    echo other > /work/tmp/EG_two/chrome/state
    ln -s /etc/passwd /work/tmp/EG_two/link.ogg
}
start() {
    JUNTO_RECORDER_WORK=/work/tmp JUNTO_RECORDER_DIR=/work/recordings JUNTO_RECORDER_IMAGE_START=/work/image-start \
        bash /deploy/images/recorder/start.sh
}

prepare
start > /work/log 2>&1; code=$?
check "the start ends with success" "[ $code -eq 0 ]"
check "the partial file of each job is saved" "[ \"\$(cat /work/recordings/rescued/EG_one/segment-a.ogg)\" = partial-one ] && [ \"\$(cat /work/recordings/rescued/EG_two/segment-b.ogg)\" = partial-two ]"
check "a link is not saved" "[ ! -e /work/recordings/rescued/EG_two/link.ogg ]"
check "the start script of the image ran afterwards" "[ -e /work/image-started ]"

prepare
chmod 0555 /work/recordings
start > /work/log 2>&1; code=$?
check "a copy that fails stops the start" "[ $code -ne 0 ]"
check "the start script of the image did not run" "[ ! -e /work/image-started ]"
check "the partial files are still in the temporary directory" "[ -s /work/tmp/EG_one/segment-a.ogg ] && [ -s /work/tmp/EG_two/segment-b.ogg ]"
check "the message names the directory" "grep -q rescued /work/log"
chmod 0755 /work/recordings

rm -rf /work/tmp /work/image-started
start > /work/log 2>&1; code=$?
check "a start without a temporary directory runs the start script of the image" "[ $code -eq 0 ] && [ -e /work/image-started ]"

if [ "$failures" -eq 0 ]; then echo "all checks passed"; else echo "$failures check(s) failed"; exit 1; fi
'
