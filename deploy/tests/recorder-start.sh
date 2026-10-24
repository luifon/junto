#!/bin/sh
# Tests the start script of the recorder container (deploy/images/recorder/start.sh) in the
# server image, which has bash: partial audio files are copied to rescued/<job>/ before the
# start script of the image runs, and a copy that fails stops the start, so that the start
# script of the image does not delete the only copy. At a stop of the container the script
# passes SIGTERM on to the recorder when no job is left, after the wait for the jobs, and
# SIGINT when a job still runs after it (ADR 0601).
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

# A stand-in for the recorder: it starts one job process, "sleep <seconds>", when it is given
# a number, and writes the signal that ends it.
cat > /work/recorder <<"EOF"
#!/bin/bash
job=""
if [ -n "${JOB_SECONDS:-}" ]; then
    sleep "$JOB_SECONDS" &
    job=$!
fi
trap "echo TERM > /work/signal; kill \$job 2>/dev/null; exit 0" TERM
trap "echo INT > /work/signal; kill \$job 2>/dev/null; exit 0" INT
touch /work/up
while :; do sleep 0.2; done
EOF
chmod 755 /work/recorder
# Starts the script with the stand-in, sends it SIGTERM as Docker does, and prints the signal
# that the stand-in got, the seconds until the script ended, and its exit code.
stopped() {
    rm -f /work/signal /work/up
    JOB_SECONDS=$1 JUNTO_RECORDER_JOB_COMMAND="sleep $2" JUNTO_RECORDER_STOP_WAIT=$3 \
        JUNTO_RECORDER_WORK=/work/none JUNTO_RECORDER_DIR=/work/recordings JUNTO_RECORDER_IMAGE_START=/work/recorder \
        bash /deploy/images/recorder/start.sh > /work/log 2>&1 &
    pid=$!
    tries=0
    while [ ! -e /work/up ] && [ "$tries" -lt 100 ]; do tries=$((tries + 1)); sleep 0.1; done
    started=$(date +%s)
    kill -TERM "$pid"
    wait "$pid"
    code=$?
    echo "$(cat /work/signal 2>/dev/null) $(($(date +%s) - started)) $code"
}
set -- $(stopped "" 99 6)
check "without a job the recorder gets SIGTERM at once" "[ \"$1\" = TERM ] && [ $2 -le 1 ] && [ $3 -eq 0 ]"
set -- $(stopped 3 3 6)
check "with a job that ends within the wait the recorder gets SIGTERM after the job ended" "[ \"$1\" = TERM ] && [ $2 -ge 2 ] && [ $2 -le 5 ] && [ $3 -eq 0 ]"
set -- $(stopped 30 30 2)
check "with a job that runs after the wait the recorder gets SIGINT at the end of the wait" "[ \"$1\" = INT ] && [ $2 -ge 2 ] && [ $2 -le 4 ] && [ $3 -eq 0 ]"

if [ "$failures" -eq 0 ]; then echo "all checks passed"; else echo "$failures check(s) failed"; exit 1; fi
'
