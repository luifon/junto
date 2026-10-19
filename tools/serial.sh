#!/bin/sh
# Runs one heavy command at a time on this machine.
#
# Every caller takes the same lock file, whatever TMPDIR holds. A caller that sets TMPDIR
# for its own temporary files would otherwise take a lock of its own and run beside the
# others. On macOS the file is in the temporary directory of the user as the system states
# it; on other systems it is in /tmp. JUNTO_HEAVY_LOCK names another file, for the test of
# a script that calls this one, and for the run lock of a CI runner. Without lockf and flock
# the lock is taken by tools/windows/lock.py.
#
# serial.sh --inherited <command> takes the lock with tools/windows/lock.py on every system:
# the processes of the command inherit the locked file, so the lock lasts until the last of
# them has ended, and TERM, INT and HUP go to the command (tools/ci/run-jobs.sh). BSD lockf
# frees the lock when its direct child ends; it and lock.py take the same lock of the file.
inherited=false
if [ "${1:-}" = --inherited ]; then
    inherited=true
    shift
fi
if [ -n "${JUNTO_HEAVY_LOCK:-}" ]; then
    lock="$JUNTO_HEAVY_LOCK"
else
    directory=$(getconf DARWIN_USER_TEMP_DIR 2>/dev/null) || directory=""
    [ -n "$directory" ] || directory=/tmp
    lock="${directory%/}/junto-heavy.lock"
fi
if ! "$inherited" && command -v lockf >/dev/null 2>&1; then
    exec lockf -k "$lock" nice -n 10 "$@"
fi
if ! "$inherited" && command -v flock >/dev/null 2>&1; then
    exec flock "$lock" nice -n 10 "$@"
fi
# The Bash of Git for Windows has neither program.
exec python3 "$(dirname -- "$0")/windows/lock.py" "$lock" nice -n 10 "$@"
