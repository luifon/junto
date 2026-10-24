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
if [ "${1:-}" = --machine-gate ]; then
    shift
    export JUNTO_MACHINE_GATE_HELD=1
    exec python3 "$(dirname -- "$0")/windows/lock.py" /tmp/junto-heavy-machine.lock "$@"
fi
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
host_cpus=16
if [ "$(uname -s)" = Linux ]; then
    host_cpus=$(getconf _NPROCESSORS_ONLN 2>/dev/null) || host_cpus=1
    case "$host_cpus" in ''|*[!0-9]*|0) host_cpus=1 ;; esac
    if [ "$host_cpus" -lt 16 ] && [ "${JUNTO_MACHINE_GATE_HELD:-}" != 1 ]; then
        # Take the caller's lock first, then the shared gate. With --inherited, descendants
        # retain both descriptors (JUNTO_LOCK_FD of tools/windows/lock.py). Descendants may take
        # their private locks without taking the gate again.
        set -- sh "$0" --machine-gate "$@"
    fi
fi
# The four worker locks partition CPUs 0-7 on the Linux host; CI reserves CPUs 8-15.
# An explicit first CPU selects another allocation, including on a smaller host.
if [ "$(uname -s)" = Linux ] && [ -z "${JUNTO_LINUX_CPU_FIRST:-}" ]; then
    first=""
    case "$lock" in
        /tmp/junto-heavy.lock) first=0 ;;
        /tmp/junto-heavy-2.lock) first=2 ;;
        /tmp/junto-heavy-3.lock) first=4 ;;
        /tmp/junto-heavy-4.lock) first=6 ;;
    esac
    if [ -n "$first" ]; then
        [ "$host_cpus" -ge 16 ] || first=0
        export JUNTO_LINUX_CPU_FIRST=$first
        case "${JUNTO_LINUX_CPUS:-2}" in
            ''|*[!0-9]*|0) ;;
            1) export JUNTO_LINUX_CPUS=1 ;;
            *) export JUNTO_LINUX_CPUS=2 ;;
        esac
    fi
fi
if ! "$inherited" && command -v lockf >/dev/null 2>&1; then
    exec lockf -k "$lock" nice -n 10 "$@"
fi
if ! "$inherited" && command -v flock >/dev/null 2>&1; then
    exec flock "$lock" nice -n 10 "$@"
fi
# The Bash of Git for Windows has neither program.
exec python3 "$(dirname -- "$0")/windows/lock.py" "$lock" nice -n 10 "$@"
