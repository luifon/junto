#!/bin/sh
# The job "Transcription worker" (ADR 0905): Python 3.12 and the locked packages with uv of
# tools/ci/pins.sh, ruff, mypy and the unit tests.
set -eu
cd "$(dirname "$0")/../.."
. tools/ci/pins.sh

uv_version=$(uv --version | cut -d ' ' -f 2)
if [ "$uv_version" != "$JUNTO_CI_UV_VERSION" ]; then
    echo "uv $uv_version; this job needs $JUNTO_CI_UV_VERSION" >&2
    exit 1
fi
cd workers/transcription
printf '\n=== Install Python 3.12 and the locked packages\n'
uv sync --frozen
printf '\n=== Lint\n'
uv run --frozen ruff check src tests
uv run --frozen ruff format --check src tests
printf '\n=== Types\n'
uv run --frozen mypy
printf '\n=== Unit tests\n'
uv run --frozen pytest
