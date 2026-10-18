#!/bin/sh
# Keeps the caches of a CI runner within their limits (ADR 0905):
#
#   caches.sh <directory> <runner> before|after
#
# <directory> is the CI directory of the machine and <runner> "linux", "macos" or "windows".
# tools/ci/run-jobs.sh runs it with the run lock held, before the first job of a run and after
# its last job, so no job uses a cache while it runs.
#
# The caches: on the Linux host the Docker volumes junto-ci-target (Cargo target directory),
# junto-ci-cargo (Cargo registry), junto-ci-uv and junto-ci-npm; on macOS and on Windows the
# Cargo target directory <directory>/cargo-target, and on Windows also the Cargo registry
# <directory>/cargo/registry. A cache above its size cap is emptied. Then, while the free
# space of the disk that holds the caches is below the free-space level, the largest cache
# that is not empty is emptied. A run fills an emptied cache again from nothing.
#
# The limits are in GiB. Each is read from a line NAME=VALUE of <directory>/settings, and has
# its default when the file has no such line:
#
#   JUNTO_CI_TARGET_CAP_GIB=60   the Cargo target directory
#   JUNTO_CI_CARGO_CAP_GIB=8     the Cargo registry
#   JUNTO_CI_UV_CAP_GIB=4        the uv cache and its Python
#   JUNTO_CI_NPM_CAP_GIB=4       the npm cache and the Playwright browser
#   JUNTO_CI_FREE_GIB=60         the free-space level of the disk
#
# Exit status 1 when the free space is below the level before a run after every cache was
# emptied: the run then starts no job. Each measurement and each removal is written to the
# standard output; the reason for a status 1 to the standard error.
set -eu
if [ "$#" -ne 3 ]; then
    echo "usage: $0 <directory> <runner> before|after" >&2
    exit 2
fi
directory=$1 runner=$2 phase=$3
case "$runner" in linux | macos | windows) ;; *) echo "no runner $runner" >&2; exit 2 ;; esac
case "$phase" in before | after) ;; *) echo "no phase $phase" >&2; exit 2 ;; esac
. "$(dirname -- "$0")/pins.sh"

limit() {
    value=$(sed -n "s/^$1=//p" "$directory/settings" 2>/dev/null | tail -n 1)
    case "$value" in
        '') value=$2 ;;
        *[!0-9]*) echo "$directory/settings: $1 is not a whole number of GiB" >&2; exit 2 ;;
    esac
    echo $((value * 1024 * 1024))
}
target_cap=$(limit JUNTO_CI_TARGET_CAP_GIB 60)
cargo_cap=$(limit JUNTO_CI_CARGO_CAP_GIB 8)
uv_cap=$(limit JUNTO_CI_UV_CAP_GIB 4)
npm_cap=$(limit JUNTO_CI_NPM_CAP_GIB 4)
level=$(limit JUNTO_CI_FREE_GIB 60)

# The caches of the runner, one per line: name and cap in KiB.
case "$runner" in
    linux) caches="junto-ci-target $target_cap
junto-ci-cargo $cargo_cap
junto-ci-uv $uv_cap
junto-ci-npm $npm_cap" ;;
    macos) caches="cargo-target $target_cap" ;;
    windows) caches="cargo-target $target_cap
cargo/registry $cargo_cap" ;;
esac

# The Docker volumes are measured and emptied in a container, which also reads the free space
# of the disk of Docker's volumes.
# in_volume <purpose> <volume> <command>...
in_volume() {
    container=junto-ci-cache-$1 volume=$2
    shift 2
    docker run --rm --name "$container" --network none --cpus 1 --memory 256m \
        --memory-swap 256m -v "$volume:/cache" "$JUNTO_CI_PYTHON_IMAGE" "$@"
}
# size <cache>: the size of a cache in KiB, empty when the cache does not exist.
size() {
    if [ "$runner" = linux ]; then
        docker volume inspect "$1" >/dev/null 2>&1 || return 0
        in_volume size "$1" du -sk /cache | cut -f 1
    elif [ -d "$directory/$1" ]; then
        python3 "$(dirname -- "$0")/cache_size.py" "$directory/$1"
    fi
}
# free: the free space in KiB of the disk that holds the caches.
free() {
    if [ "$runner" = linux ]; then
        in_volume free junto-ci-target df -Pk /cache | awk 'NR == 2 { print $4 }'
    else
        df -Pk "$directory" | awk 'NR == 2 { print $4 }'
    fi
}
empty() {
    if [ "$runner" = linux ]; then
        in_volume empty "$1" find /cache -mindepth 1 -delete
    else
        rm -rf "${directory:?}/$1"
    fi
}
log() { printf '%s  %s: %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$phase" "$1"; }

# The caches that remain after the caps, with their sizes: size, name.
remaining=""
while read -r name cap; do
    kib=$(size "$name")
    if [ -z "$kib" ]; then continue; fi
    if [ "$kib" -gt "$cap" ]; then
        empty "$name"
        log "cache $name: $kib KiB, above its cap of $cap KiB; emptied"
    else
        log "cache $name: $kib KiB of $cap KiB"
        remaining="$remaining$kib $name
"
    fi
done <<EOF
$caches
EOF

available=$(free)
log "free space: $available KiB, level $level KiB"
for name in $(printf '%s' "$remaining" | sort -rn | cut -d ' ' -f 2); do
    if [ "$available" -ge "$level" ]; then break; fi
    empty "$name"
    available=$(free)
    log "cache $name emptied for free space; free space: $available KiB"
done
if [ "$phase" = before ] && [ "$available" -lt "$level" ]; then
    echo "the run starts no job: the disk of the caches has $available KiB free, below the level of $level KiB (JUNTO_CI_FREE_GIB in $directory/settings)" >&2
    exit 1
fi
