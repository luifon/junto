#!/bin/sh
# Starts or stops the local media stack. Creates .env.local with a generated key on first use.
set -eu
cd "$(dirname "$0")/.."
if [ ! -f .env.local ]; then
  umask 077
  {
    echo "LIVEKIT_URL=ws://localhost:7880"
    echo "LIVEKIT_API_KEY=API$(openssl rand -hex 6)"
    echo "LIVEKIT_API_SECRET=$(openssl rand -hex 32)"
  } > .env.local
fi
mkdir -p out
exec docker compose --env-file .env.local "$@"
