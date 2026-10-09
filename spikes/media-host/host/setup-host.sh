#!/bin/sh
set -eu
cd /opt/junto-spike
umask 077
if [ ! -f .env ]; then
  node_ip=$(ip -o -4 route get 1.1.1.1 | sed -n 's/.* src \([0-9.]*\).*/\1/p')
  {
    echo "NODE_IP=$node_ip"
    echo "LK_API_KEY=API$(openssl rand -hex 6)"
    echo "LK_API_SECRET=$(openssl rand -base64 36 | tr -d '/+=\n')"
    echo "REDIS_PASSWORD=$(openssl rand -hex 24)"
  } > .env
fi
chmod 600 .env
chmod 644 livekit.yaml compose.yaml
docker compose config --quiet
