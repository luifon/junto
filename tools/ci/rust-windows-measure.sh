#!/bin/sh
# The optional Windows job rust-windows-measure: the measurements that need the desktop of the
# Windows machine, printed as tables in its log. It checks nothing that rust-windows does not
# check, and does not decide whether a commit passed (ADR 0905, ADR 0914).
set -eu
cd "$(dirname "$0")/../.."
# The processor time of a window share and of an application share (ADR 0738), in a release
# build, as the client is built for a release (ADR 0914).
JUNTO_TEST_SHARE_COST=1 cargo test --release --locked -p junto-media --features test-support \
    --test windows_capture -- the_processor_time_of_an_application_share --exact --nocapture
