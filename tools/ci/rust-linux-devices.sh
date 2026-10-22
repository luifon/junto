#!/bin/sh
# Explicit native camera and synthetic live-tier job (ADR 0905, ADR 0913).
set -eu
cd "$(dirname "$0")/../.."
. tools/ci/pins.sh

tools/linux-container.sh cargo clippy --locked -p junto-media --lib -- -D warnings
tools/linux-container.sh cargo clippy --locked -p junto-media --all-targets -- -D warnings
tools/linux-container.sh cargo build --locked --workspace
tools/linux-container.sh cargo test --locked -p junto-media --lib --test camera_factory
tools/linux-container.sh cargo test --locked -p junto-media --no-fail-fast -- --ignored
tools/linux-container.sh cargo test --locked -p junto-client --test app live:: -- --ignored
JUNTO_LINUX_CAMERA=1 tools/linux-container.sh \
    cargo run --locked -p junto-media --features native-device-tests --example camera-device
