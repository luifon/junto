#!/bin/sh
# The server tests expect a release path; the container keeps one debug build profile.
set -eu
cargo build --locked --manifest-path spikes/test-participant/Cargo.toml \
    --config 'profile.dev.debug="line-tables-only"' \
    --config 'profile.dev.package."*".debug=false'
participant=spikes/test-participant/target/release/test-participant
mkdir -p spikes/test-participant/target/release
if [ ! -e "$participant" ] && [ ! -L "$participant" ]; then
    ln -s ../debug/test-participant "$participant"
fi
if [ ! -L "$participant" ] || [ "$(readlink "$participant")" != ../debug/test-participant ]; then
    echo "the participant path must point to the debug build" >&2
    exit 1
fi
exec cargo test -p junto-server --test api live:: -- --ignored "$@"
