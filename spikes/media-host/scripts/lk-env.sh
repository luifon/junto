#!/bin/sh
repo_root=$(cd "$(dirname "$0")/../../.." && pwd)
set -a
. "$repo_root/.env.local"
set +a
export LIVEKIT_URL="$JUNTO_TEST_LK_URL"
export LIVEKIT_API_KEY="$JUNTO_TEST_LK_API_KEY"
export LIVEKIT_API_SECRET="$JUNTO_TEST_LK_API_SECRET"
exec "$@"
