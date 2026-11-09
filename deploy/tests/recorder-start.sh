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

# The checks of the stop follow the order of events and not the clock, so that a slow
# scheduler cannot fail a correct script. The script runs with a sleep in front of the real
# one that records each second of its wait; the stand-in and the job use the real sleep.
real_sleep=$(command -v sleep)
mkdir -p /work/bin
printf "#!/bin/bash\necho waited >> /work/events\nexec %s \"\$@\"\n" "$real_sleep" > /work/bin/sleep
chmod 755 /work/bin/sleep
# A stand-in for the recorder: it starts one job process, "sleep 1000", when JOB is set, and
# records the signal that ends it.
cat > /work/recorder <<EOF
#!/bin/bash
job=""
if [ -n "\${JOB:-}" ]; then
    $real_sleep 1000 &
    job=\$!
    echo "\$job" > /work/job
fi
trap "echo TERM >> /work/events; kill \\\$job 2>/dev/null; exit 0" TERM
trap "echo INT >> /work/events; kill \\\$job 2>/dev/null; exit 0" INT
touch /work/up
while :; do $real_sleep 0.2; done
EOF
chmod 755 /work/recorder
# stopped <job> <stop wait> <end the job>: starts the script with the stand-in, sends it
# SIGTERM as Docker does, ends the job once the script waits when the third argument is
# "end", and prints the events, one word each: "waited" for each second of the wait,
# "ended" just before the test ends the job, and the signal of the recorder; then the exit
# code of the script.
stopped() {
    rm -f /work/events /work/up /work/job
    touch /work/events
    JOB=$1 JUNTO_RECORDER_JOB_COMMAND="$real_sleep 1000" JUNTO_RECORDER_STOP_WAIT=$2 PATH="/work/bin:$PATH" \
        JUNTO_RECORDER_WORK=/work/none JUNTO_RECORDER_DIR=/work/recordings JUNTO_RECORDER_IMAGE_START=/work/recorder \
        bash /deploy/images/recorder/start.sh > /work/log 2>&1 &
    pid=$!
    # Each wait for an event has a bound of 300 steps of 0.1 s: a script that never starts
    # the recorder, never waits or never ends is stated as such and does not hold the job.
    steps=0
    until [ -e /work/up ] || ! kill -0 "$pid" 2>/dev/null; do
        steps=$((steps + 1))
        if [ "$steps" -gt 300 ]; then echo no-start >> /work/events; break; fi
        "$real_sleep" 0.1
    done
    kill -TERM "$pid"
    if [ "$3" = end ]; then
        steps=0
        until grep -q waited /work/events || ! kill -0 "$pid" 2>/dev/null; do
            steps=$((steps + 1))
            if [ "$steps" -gt 300 ]; then echo no-wait >> /work/events; break; fi
            "$real_sleep" 0.1
        done
        if grep -q waited /work/events; then
            echo ended >> /work/events
            kill "$(cat /work/job)"
        fi
    fi
    steps=0
    while kill -0 "$pid" 2>/dev/null; do
        steps=$((steps + 1))
        if [ "$steps" -gt 300 ]; then echo no-end >> /work/events; kill -KILL "$pid"; break; fi
        "$real_sleep" 0.1
    done
    wait "$pid"
    code=$?
    echo $(cat /work/events) "$code"
}
check "without a job the recorder gets SIGTERM without a wait" "[ \"$(stopped "" 60 keep)\" = \"TERM 0\" ]"
check "it states the stop: no job at the signal and SIGTERM after 0 s" \
    "grep -q \"^recorder stop: jobs running at the stop signal: 0; the wait for them is 60 s\$\" /work/log && grep -q \"^recorder stop: no job runs after 0 s; SIGTERM to the recorder\$\" /work/log"
events=$(stopped 1 60 end)
check "with a job that ends within the wait the recorder gets SIGTERM after the job ended: $events" \
    "case \"$events\" in \"waited \"*\"ended \"*\"TERM 0\") true ;; *) false ;; esac"
check "it states one job at the signal and SIGTERM after the wait" \
    "grep -q \"^recorder stop: jobs running at the stop signal: 1;\" /work/log && grep -q \"^recorder stop: no job runs after [1-9][0-9]* s; SIGTERM\" /work/log"
events=$(stopped 1 2 keep)
check "with a job that runs after the wait the recorder gets SIGINT at the end of the wait: $events" \
    "[ \"$events\" = \"waited waited INT 0\" ]"
check "it states the job that still runs and SIGINT" "grep -q \"^recorder stop: jobs still running after 2 s: 1; SIGINT to the recorder\$\" /work/log"

if [ "$failures" -eq 0 ]; then echo "all checks passed"; else echo "$failures check(s) failed"; exit 1; fi
'
