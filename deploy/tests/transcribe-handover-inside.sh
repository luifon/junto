#!/bin/bash
# shellcheck disable=SC2034 # The checks read variables through eval.
# Runs inside the server image as root (see transcribe-handover.sh). The hand-over script
# runs as the user of the server (1001, with the groups 10001 and 10002). The loop runs as
# in the worker container: as root with the group of the spool (10002) and the capabilities
# SETUID, SETGID and KILL only, and it runs each job as the user of the worker (10001).
set -u

handover=/deploy/images/server/junto-transcribe-handover
loop=/deploy/images/worker/junto-worker-loop
server_user="setpriv --reuid 1001 --regid 1001 --groups 1001,10001,10002"
worker_user="setpriv --reuid 10001 --regid 10001 --clear-groups"
loop_user="setpriv --regid 10002 --clear-groups --inh-caps=-all --bounding-set=-all,+setuid,+setgid,+kill"
work=/work
spool=$work/spool
recordings=$work/recordings
failures=0
loop_pid=""

check() {
    if eval "$2"; then
        echo "ok    $1"
    else
        echo "FAIL  $1"
        failures=$((failures + 1))
    fi
}

# The stand-in worker: what it does is the content of the file "behaviour" beside the job.
cat > $work/stand-in <<'STANDIN'
#!/bin/bash
job=$2
transcript=$4
behaviour=$(cat "$(dirname "$job")/behaviour" 2>/dev/null)
echo "segment 1 of 1: status=ok" >&2
case "$behaviour" in
    exit-3) echo "model_file_missing: whisper" >&2; exit 3 ;;
    wait)
        trap 'echo ended-by-signal >&2; exit 143' TERM
        echo started > "$(dirname "$job")/started"
        for _ in $(seq 1 600); do sleep 0.1; done
        exit 0
        ;;
    forge)
        # A job that tries to state its own end, to stop its supervisor and to read the spool.
        for result in /work/spool/*.result; do echo "exit 0" >> "$result"; done 2>/dev/null
        ls /work/spool > "$(dirname "$job")/spool-listing" 2>/dev/null
        kill -STOP "$PPID" 2>/dev/null
        for pid in $(ls /proc | grep '^[0-9]*$'); do
            [ "$pid" = "$$" ] || kill -STOP "$pid" 2>/dev/null
        done
        sleep 2
        exit 5
        ;;
    child)
        # A child in a session of its own that ignores the stop signal.
        setsid bash -c 'trap "" TERM; exec -a stand-in-child sleep 600' &
        trap 'exit 143' TERM
        echo started > "$(dirname "$job")/started"
        for _ in $(seq 1 600); do sleep 0.1; done
        ;;
    *) echo '{"format_version": 1}' > "$transcript" ;;
esac
STANDIN
chmod 755 $work/stand-in

prepare() {
    rm -rf "$spool" "$recordings"
    install -d -o 1001 -g 10002 -m 0750 "$spool"
    install -d -o 1001 -g 10001 -m 0750 "$recordings"
}

# Creates the directory of one run as the server does, with the recorder's file beside it.
new_run() {
    local recording=$1 behaviour=$2 run
    run=$recordings/$recording/transcription/1-$RANDOM
    $server_user bash -c "umask 022; mkdir -p '$run' && echo '{}' > '$run/job.json' \
        && echo audio > '$recordings/$recording/segment.ogg' && echo '$behaviour' > '$run/behaviour'"
    echo "$run"
}

start_loop() {
    JUNTO_WORKER_SPOOL=$spool JUNTO_WORKER_RECORDINGS=$recordings JUNTO_WORKER_PROGRAM=$work/stand-in \
        JUNTO_WORKER_POLL_SECONDS=0.2 JUNTO_WORKER_END_SECONDS=3 JUNTO_WORKER_JOB_USER=10001:10001 \
        $loop_user bash "$loop" 2>/dev/null &
    loop_pid=$!
}

stop_loop() {
    [ -z "$loop_pid" ] || { kill "$loop_pid" 2>/dev/null; wait "$loop_pid" 2>/dev/null; }
    loop_pid=""
}

hand_over() {
    local run=$1
    shift
    # exec: the caller ends this process by its identifier when it runs in the background.
    exec_or_run=${HAND_OVER_EXEC:+exec}
    $exec_or_run $server_user bash "$handover" --spool "$spool" --poll-seconds 0.1 "$@" \
        run "$run/job.json" --out "$run/transcript.json" --audio-root "$(dirname "$(dirname "$run")")"
}

# True while a process whose command line contains the text runs.
running() {
    local file
    for file in /proc/[0-9]*/cmdline; do
        [ "$file" != "/proc/$$/cmdline" ] || continue
        if { tr '\0' ' ' < "$file"; } 2>/dev/null | grep -q -- "$1"; then
            return 0
        fi
    done
    return 1
}

wait_for() {
    local tries=0
    until eval "$1"; do
        tries=$((tries + 1))
        [ "$tries" -lt 100 ] || return 1
        sleep 0.1
    done
}

echo "# a job that the worker finishes"
prepare; start_loop
run=$(new_run rec-a ok)
hand_over "$run" 2> $work/log; code=$?
check "the exit code of the worker is passed on" '[ "$code" -eq 0 ]'
check "the log of the worker is passed on" 'grep -q "segment 1 of 1: status=ok" $work/log'
check "the worker wrote the transcript into the directory of the run" '[ -s "$run/transcript.json" ]'
check "the transcript belongs to the user of the worker" '[ "$(stat -c %u "$run/transcript.json")" = 10001 ]'
check "the user of the server reads the transcript" '$server_user cat "$run/transcript.json" > /dev/null'
check "the directory of the run has the mode 2770" '[ "$(stat -c %a "$run")" = 2770 ]'
check "the spool is empty afterwards" '[ -z "$(ls -A "$spool")" ]'
check "the worker cannot write to an audio file" '! $worker_user bash -c "echo x >> $recordings/rec-a/segment.ogg" 2>/dev/null'
check "the worker cannot create a file beside the audio files" '! $worker_user touch $recordings/rec-a/new 2>/dev/null'
check "the worker cannot create a file in the spool" '! $worker_user touch $spool/new.request 2>/dev/null'
check "the worker cannot list the spool" '! $worker_user ls $spool > /dev/null 2>&1'
check "the worker reads an audio file" '$worker_user cat $recordings/rec-a/segment.ogg > /dev/null'

echo "# a job that the worker refuses"
run=$(new_run rec-b exit-3)
hand_over "$run" 2> $work/log; code=$?
check "the exit code 3 is passed on" '[ "$code" -eq 3 ]'
check "the reason is passed on" 'grep -q "model_file_missing" $work/log'

echo "# a stop signal ends the worker"
run=$(new_run rec-c wait)
HAND_OVER_EXEC=1 hand_over "$run" 2> $work/log &
handover_pid=$!
check "the worker started the job" 'wait_for "[ -e $run/started ]"'
kill -TERM "$handover_pid"
wait "$handover_pid"; code=$?
check "the hand-over ends with 143" '[ "$code" -eq 143 ]'
check "the worker process got the signal" 'grep -q ended-by-signal $work/log'
check "no stand-in process is left" '! running "stand-in run"'
check "the spool is empty afterwards" '[ -z "$(ls -A "$spool")" ]'

echo "# a request of a server process that no longer waits"
run=$(new_run rec-d wait)
HAND_OVER_EXEC=1 hand_over "$run" 2> /dev/null &
handover_pid=$!
check "the first job runs" 'wait_for "[ -e $run/started ]"'
kill -KILL "$handover_pid"
wait "$handover_pid" 2>/dev/null
check "the request stays in the spool" '[ -n "$(ls "$spool"/*.request 2>/dev/null)" ]'
second=$(new_run rec-e ok)
hand_over "$second" 2> $work/log; code=$?
check "the next job ends with 0" '[ "$code" -eq 0 ]'
check "the first job was ended" 'wait_for "! running \"stand-in run\""'
third=$(new_run rec-f ok)
hand_over "$third" 2> $work/log; code=$?
check "a later hand-over removes the finished request" '[ "$code" -eq 0 ] && [ -z "$(ls -A "$spool")" ]'
stop_loop

echo "# a job whose child leaves its process group"
prepare; start_loop
run=$(new_run rec-h child)
HAND_OVER_EXEC=1 hand_over "$run" 2> $work/log &
handover_pid=$!
check "the job with a child runs" 'wait_for "[ -e $run/started ]" && wait_for "running stand-in-child"'
kill -TERM "$handover_pid"
wait "$handover_pid"; code=$?
check "the hand-over ends with 143" '[ "$code" -eq 143 ]'
check "the child of the job is ended too" 'wait_for "! running stand-in-child"'
run=$(new_run rec-i child)
HAND_OVER_EXEC=1 hand_over "$run" 2> $work/log &
handover_pid=$!
check "a second job with a child runs" 'wait_for "[ -e $run/started ]" && wait_for "running stand-in-child"'
stop_loop
kill -TERM "$handover_pid" 2>/dev/null; wait "$handover_pid" 2>/dev/null
check "a stop of the loop ends the child of the job" 'wait_for "! running stand-in-child"'

echo "# a job that tries to state its own end and to stop the loop"
prepare; start_loop
run=$(new_run rec-l forge)
hand_over "$run" 2> $work/log; code=$?
check "the exit code is the one that the loop saw, not the one that the job wrote" '[ "$code" -eq 5 ]'
check "the job did not read the spool" '[ ! -s "$run/spool-listing" ]'
run=$(new_run rec-m ok)
hand_over "$run" --take-seconds 10 2> $work/log; code=$?
check "the loop runs the next job" '[ "$code" -eq 0 ]'
stop_loop

echo "# a result that the worker fills with other bytes"
prepare
run=$(new_run rec-j ok)
HAND_OVER_EXEC=1 hand_over "$run" 2> $work/log &
handover_pid=$!
check "the request is in the spool" 'wait_for "ls $spool/*.result > /dev/null 2>&1"'
result=$(ls "$spool"/*.result)
{ echo taken; head -c 16384 /dev/zero | tr '\0' x; echo; echo 'exit 0'; } >> "$result"
wait "$handover_pid"; code=$?
check "a result that is larger than a result can be ends the hand-over with 75" '[ "$code" -eq 75 ]'
check "the request is cancelled" '[ -n "$(ls "$spool"/*.cancel 2>/dev/null)" ]'

echo "# no worker"
prepare
run=$(new_run rec-g ok)
hand_over "$run" --take-seconds 1 2> $work/log; code=$?
check "the hand-over ends with 75" '[ "$code" -eq 75 ]'
check "the message names the worker container" 'grep -q "no worker took the job" $work/log'
check "the request stays in the spool with a cancel file" '[ -n "$(ls "$spool"/*.request 2>/dev/null)" ] && [ -n "$(ls "$spool"/*.cancel 2>/dev/null)" ]'
start_loop
check "a loop that starts later ends the request without running it" 'wait_for "grep -q \"^exit 143\" $spool/*.result" && [ ! -e "$run/transcript.json" ]'
stop_loop

echo "# a stop signal before a worker took the job"
prepare
run=$(new_run rec-k ok)
HAND_OVER_EXEC=1 hand_over "$run" --stop-seconds 1 2> $work/log &
handover_pid=$!
check "the request is in the spool" 'wait_for "ls $spool/*.request > /dev/null 2>&1"'
kill -TERM "$handover_pid"
wait "$handover_pid"; code=$?
check "the hand-over ends with 143" '[ "$code" -eq 143 ]'
check "the request stays in the spool with a cancel file" '[ -n "$(ls "$spool"/*.request 2>/dev/null)" ] && [ -n "$(ls "$spool"/*.cancel 2>/dev/null)" ]'
start_loop
check "a loop that starts later does not run the job" 'wait_for "grep -q \"^exit 143\" $spool/*.result" && [ ! -e "$run/transcript.json" ]'
stop_loop

echo "# a path outside the recordings"
prepare; start_loop
install -d -o 1001 -g 10001 -m 0750 "$work/elsewhere"
outside=$work/elsewhere/outside
$server_user mkdir "$outside"
$server_user bash -c "echo '{}' > $outside/job.json"
$server_user bash "$handover" --spool "$spool" --poll-seconds 0.1 \
    run "$outside/job.json" --out "$outside/transcript.json" --audio-root "$outside" 2> $work/log; code=$?
check "the loop refuses the request with 2" '[ "$code" -eq 2 ]'
check "the stand-in did not run" '[ ! -e "$outside/transcript.json" ]'
stop_loop

echo "# wrong arguments"
$server_user bash "$handover" run relative.json --out relative.out --audio-root . 2> $work/log; code=$?
check "a relative path is refused with 2" '[ "$code" -eq 2 ]'
$server_user bash "$handover" --spool /nonexistent run /a/job.json --out /a/t.json --audio-root /a 2> $work/log; code=$?
check "a missing spool ends with 75" '[ "$code" -eq 75 ]'

echo
if [ "$failures" -eq 0 ]; then
    echo "all checks passed"
else
    echo "$failures check(s) failed"
    exit 1
fi
