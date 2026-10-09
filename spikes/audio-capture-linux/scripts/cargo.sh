#!/bin/bash
# Usage: cargo.sh <cargo args...>   Runs cargo for the spike inside the build container.
. "$(dirname "$0")/env.sh"
mkdir -p "$SPIKE_ROOT/cargo-registry" "$SPIKE_ROOT/src"
exec docker run --rm --name junto-spike-cargo --cpus 6 \
  -v "$SPIKE_ROOT/src":/work -v "$SPIKE_ROOT/cargo-registry":/usr/local/cargo/registry \
  -w /work junto-spike-build:1 nice -n 10 cargo "$@"
