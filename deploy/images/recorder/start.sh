#!/usr/bin/env bash
# The start script of the recorder container (ADR 0602). The start script of the image
# deletes the temporary directory of the recorder at every start. This script runs first and
# copies the partial audio files that an ended recorder left there to
# <recordings>/rescued/<job>/<file>, where the server looks for them (ADR 0606).
#
# When a copy fails, the script ends without starting the recorder, so that the start script
# of the image does not delete the only copy. The container then starts again until the
# cause is removed, for example a full disk.
#
# The script stays the parent of the recorder and passes the stop of the container on
# (ADR 0601). On SIGTERM the recorder waits until each job has ended, and only the server
# ends a job that records. On SIGINT it ends each job and finishes its files, and reports a
# job that the server had stopped and that was still finishing as failed. So after the stop
# signal the recorder gets no signal while its jobs end: a job that the server stopped has
# 20 seconds, as the server gives it (ADR 0604). Then the recorder gets SIGTERM when no job
# is left, and SIGINT when jobs still record.
set -u
work="${JUNTO_RECORDER_WORK:-/home/egress/tmp}"
recordings="${JUNTO_RECORDER_DIR:-/var/lib/junto/recordings}"
image_start="${JUNTO_RECORDER_IMAGE_START:-/entrypoint.sh}"
# The command line with which the recorder runs a job, and the wait for the jobs at a stop.
job_command="${JUNTO_RECORDER_JOB_COMMAND:-egress run-handler}"
stop_wait="${JUNTO_RECORDER_STOP_WAIT:-20}"

if [ -d "$work" ]; then
    for file in "$work"/EG_*/*.ogg; do
        [ -f "$file" ] && [ ! -L "$file" ] || continue
        job=$(basename -- "$(dirname -- "$file")")
        target="$recordings/rescued/$job"
        if ! { mkdir -p -- "$target" && cp -- "$file" "$target/" && sync -- "$target/$(basename -- "$file")"; }; then
            echo "recorder start: a partial audio file of the job $job could not be copied to $recordings/rescued. The recorder does not start, and the file stays in $work." >&2
            exit 1
        fi
    done
fi
# The number of processes that run a job of the recorder.
jobs_left() {
    local file count=0
    for file in /proc/[0-9]*/cmdline; do
        case "$(tr '\000' ' ' < "$file" 2>/dev/null)" in
            "$job_command"*) count=$((count + 1)) ;;
        esac
    done
    echo "$count"
}

recorder=""
stopping=""
stop() {
    local waited=0
    stopping=1
    [ -n "$recorder" ] || return 0
    while [ "$waited" -lt "$stop_wait" ] && [ "$(jobs_left)" -gt 0 ]; do
        sleep 1
        waited=$((waited + 1))
    done
    if [ "$(jobs_left)" -gt 0 ]; then
        kill -INT "$recorder" 2>/dev/null
    else
        kill -TERM "$recorder" 2>/dev/null
    fi
}
trap stop TERM INT
# A command that a shell starts in the background, and the child of a process that ignores
# SIGINT, ignore SIGINT; env gives the recorder the default action of that signal.
env --default-signal=INT "$image_start" &
recorder=$!
[ -z "$stopping" ] || stop
# A signal that the trap takes ends the wait; the script waits again until the recorder has
# ended, and ends with its exit code.
while :; do
    wait "$recorder"
    code=$?
    kill -0 "$recorder" 2>/dev/null || exit "$code"
done
