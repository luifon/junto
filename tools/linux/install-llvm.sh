#!/bin/sh
# Pool filenames and hashes remain independent of the repository's moving package index.
set -eu
architecture=$(dpkg --print-architecture)
case "$architecture" in arm64|amd64) ;; *) echo "unsupported architecture: $architecture" >&2; exit 1 ;; esac
manifest="/opt/junto-linux/llvm-$architecture.sha256"
directory=$(mktemp -d /tmp/junto-linux1-llvm-XXXXXX)
set --
cleanup() {
    for package in "$@"; do rm -f -- "$package"; done
    rmdir "$directory"
}
trap 'cleanup "$@"' EXIT
trap 'exit 130' INT TERM
while read -r checksum filename; do
    set -- "$@" "$directory/$filename"
    curl --retry 3 -fsSL "https://apt.llvm.org/bookworm/pool/main/l/llvm-toolchain-21/$filename" -o "$directory/$filename"
done < "$manifest"
(cd "$directory" && sha256sum -c "$manifest")
apt-get update
apt-get install -y --no-install-recommends "$@"
rm -rf /var/lib/apt/lists
