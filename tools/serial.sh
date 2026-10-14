#!/bin/sh
# Runs one heavy command at a time on this machine.
#
# Every caller takes the same lock file, whatever TMPDIR holds. A caller that sets TMPDIR
# for its own temporary files would otherwise take a lock of its own and run beside the
# others. On macOS the file is in the temporary directory of the user as the system states
# it; on other systems it is in /tmp. JUNTO_HEAVY_LOCK names another file, for the test of
# a script that calls this one.
if [ -n "${JUNTO_HEAVY_LOCK:-}" ]; then
    lock="$JUNTO_HEAVY_LOCK"
else
    directory=$(getconf DARWIN_USER_TEMP_DIR 2>/dev/null) || directory=""
    [ -n "$directory" ] || directory=/tmp
    lock="${directory%/}/junto-heavy.lock"
fi
if command -v lockf >/dev/null 2>&1; then
    exec lockf -k "$lock" nice -n 10 "$@"
fi
exec flock "$lock" nice -n 10 "$@"
