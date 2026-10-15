#!/bin/sh
# The job "Recording layout page" (ADR 0905): the locked packages without install scripts,
# the browser of the locked Playwright version with its system packages, the build, and the
# browser tests against a LiveKit server in Docker. Needs Node of tools/ci/pins.sh, Docker,
# and the permission to install system packages.
set -eu
cd "$(dirname "$0")/../.."
. tools/ci/pins.sh

node_version=$(node --version)
if [ "$node_version" != "v$JUNTO_CI_NODE_VERSION" ]; then
    echo "Node $node_version; this job needs v$JUNTO_CI_NODE_VERSION" >&2
    exit 1
fi
cd recording-layout
printf '\n=== Install the locked packages\n'
npm ci --ignore-scripts
printf '\n=== Install the browser of the locked Playwright version\n'
if [ "$(id -u)" -eq 0 ] || [ -z "${JUNTO_CI_LAYOUT_DEPS_READY:-}" ]; then
    npx --no-install playwright install --with-deps chromium
else
    npx --no-install playwright install chromium
fi
printf '\n=== Build\n'
node build.mjs
printf '\n=== Browser tests against a LiveKit server in Docker\n'
node tests/run.mjs
