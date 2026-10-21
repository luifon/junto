#!/bin/sh
# Prepare writable mounts, then run a command without root privileges.
set -eu
uid=${1:?user id required} gid=${2:?group id required}
shift 2
case "$uid:$gid" in *[!0-9:]*|:*|*:) echo 'a non-root numeric user and group are required' >&2; exit 2 ;; esac
if [ "$uid" -eq 0 ]; then
    echo 'a non-root user is required' >&2
    exit 2
fi
user_home=/tmp/junto-user-$uid
mkdir -p "$user_home"
while [ "${1:-}" != -- ]; do
    directory=${1:?writable directory or -- required}
    find "$directory" -xdev \( ! -uid "$uid" -o ! -gid "$gid" \) -exec chown -h "$uid:$gid" {} +
    shift
done
shift
if [ "$#" -eq 0 ]; then
    echo 'a command is required' >&2
    exit 2
fi
chown "$uid:$gid" "$user_home"
if [ "${JUNTO_TEST_CAMERA_DEVICE:-}" = 1 ]; then
    groups=${JUNTO_LINUX_CAMERA_GROUPS:?camera device groups required}
    case "$groups" in *[!0-9,]*|,*|*,|*,,*) echo 'numeric camera groups required' >&2; exit 2 ;; esac
    exec setpriv --reuid="$uid" --regid="$gid" --groups="$groups" env HOME="$user_home" "$@"
fi
# The layout job controls only its own test service through the mounted Docker socket.
if [ -S /var/run/docker.sock ]; then
    exec setpriv --reuid="$uid" --regid="$gid" --groups="$(stat -c %g /var/run/docker.sock)" \
        env HOME="$user_home" "$@"
fi
exec setpriv --reuid="$uid" --regid="$gid" --clear-groups env HOME="$user_home" "$@"
