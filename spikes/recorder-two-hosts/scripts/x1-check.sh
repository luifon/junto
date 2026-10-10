#!/bin/sh
# Gate X1: who can reach Redis on the public host. Run on the Mac from the spike directory.
# Addresses are replaced before anything is written.
# JUNTO_TEST_MEDIA_HOST is the SSH name of the public host, JUNTO_TEST_DATA_HOST of the data host.
set -u
here=$(cd "$(dirname "$0")/.." && pwd)
set -a; . "$here/../../.env.local"; set +a
public_ssh="${JUNTO_TEST_MEDIA_HOST:?set the SSH name of the public host}"
data_ssh="${JUNTO_TEST_DATA_HOST:?set the SSH name of the data host}"
host=$(printf '%s' "$JUNTO_TEST_LK_URL" | sed -E 's#^wss?://##; s#:[0-9]+$##')
host6=$(ssh "$public_ssh" 'ip -o -6 addr show scope global | awk "{print \$4}" | cut -d/ -f1 | head -1')
mask() { sed -E "s/$host/<public-host>/g; s/$host6/<public-host-v6>/g; s/(public key|peer): .*/\1: <key>/" | perl -pe 's/\b(?!(?:0|10|127|172|192)\.)\d{1,3}(?:\.\d{1,3}){3}\b/<address>/g'; }
{
  echo "== public host: listening sockets of the host"
  ssh "$public_ssh" 'ss -lntu | grep -E ":(6379|51820|7880|7881|7882|6789) "'
  echo "== public host: listening sockets inside the link container"
  ssh "$public_ssh" 'docker exec junto-spike-link ss -lntu | grep -v 127.0.0.11; docker port junto-spike-link; docker port junto-spike-redis'
  echo "== public host: link state"
  ssh "$public_ssh" 'docker exec junto-spike-link wg show wg0'
  echo "== public host: firewall"
  ssh "$public_ssh" 'ufw status numbered'
  echo "== data host: link state and Redis through the link"
  ssh "$data_ssh" 'docker exec junto-spike-rec-link wg show wg0; docker exec junto-spike-rec-link ping -c 3 -W 2 10.99.0.1 | tail -2'
  ssh "$data_ssh" 'cd /srv/junto-spike-rec && . ./.env && docker run --rm --name junto-spike-rec-probe --cpus 1 --memory 256m --network junto-spike-rec_default -e REDISCLI_AUTH="$REDIS_PASSWORD" redis:7.2.16-alpine sh -c "echo with password: \$(redis-cli -h link ping); echo without password: \$(REDISCLI_AUTH= redis-cli -h link ping)"'
  echo "== data host: Redis port of the public address, without the link"
  ssh "$data_ssh" "docker run --rm --name junto-spike-rec-probe --cpus 1 --memory 256m alpine:3.22 sh -c 'nc -w 5 -vz $host 6379; echo exit=\$?'"
  echo "== Mac: TCP connections to the public address"
  for port in 6379 7880 22; do nc -4 -G 5 -vz "$host" "$port"; echo "IPv4 port $port: exit $?"; done
  nc -6 -G 5 -vz "$host6" 6379; echo "IPv6 port 6379: exit $?"
  nc -6 -G 5 -vz "$host6" 22; echo "IPv6 port 22: exit $?"
} 2>&1 | mask
