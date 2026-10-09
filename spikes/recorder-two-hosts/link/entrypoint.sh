#!/bin/sh
# One end of the private link: userspace WireGuard plus one TCP forwarder.
#
#   genkey            create /keys/private (once) and print the public key
#   run               bring the link up and forward TCP
#
# Environment for "run":
#   WG_ADDRESS          this end's address on the link, with prefix length
#   WG_PEER_PUBLIC_KEY  the other end's public key
#   WG_PEER_ADDRESS     the other end's address on the link
#   WG_LISTEN_PORT      UDP port to listen on (the end that receives the connection)
#   WG_ENDPOINT         host:port of the other end (the end that opens the connection)
#   WG_KEEPALIVE        seconds between keepalive packets (the end that opens the connection)
#   FORWARD_LISTEN      address:port this end listens on
#   FORWARD_TARGET      host:port the forwarder connects to
set -eu
umask 077

case "${1:-run}" in
genkey)
  [ -s /keys/private ] || wg genkey > /keys/private
  wg pubkey < /keys/private
  exit 0
  ;;
esac

wireguard-go -f wg0 &
tunnel=$!
# Wait for the control socket of the userspace process. Do not call "wg" on
# wg0 before the socket exists: without it "wg" asks the kernel for the
# interface, and that request makes the kernel load its own wireguard module.
tries=0
until [ -S /var/run/wireguard/wg0.sock ]; do
  tries=$((tries + 1))
  [ "$tries" -lt 50 ] || { echo "the control socket of wg0 did not appear"; exit 1; }
  sleep 0.1
done

set -- set wg0 private-key /keys/private
[ -z "${WG_LISTEN_PORT:-}" ] || set -- "$@" listen-port "$WG_LISTEN_PORT"
set -- "$@" peer "$WG_PEER_PUBLIC_KEY" allowed-ips "$WG_PEER_ADDRESS/32"
[ -z "${WG_ENDPOINT:-}" ] || set -- "$@" endpoint "$WG_ENDPOINT"
[ -z "${WG_KEEPALIVE:-}" ] || set -- "$@" persistent-keepalive "$WG_KEEPALIVE"
wg "$@"

ip address add "$WG_ADDRESS" dev wg0
ip link set wg0 mtu 1420 up

listen_address=${FORWARD_LISTEN%:*}
listen_port=${FORWARD_LISTEN##*:}
socat "TCP-LISTEN:$listen_port,bind=$listen_address,fork,reuseaddr,keepalive" "TCP:$FORWARD_TARGET,connect-timeout=5,keepalive" &
forwarder=$!

trap 'kill $tunnel $forwarder 2>/dev/null; exit 0' TERM INT
while kill -0 "$tunnel" 2>/dev/null && kill -0 "$forwarder" 2>/dev/null; do
  sleep 1
done
echo "a link process ended"
kill "$tunnel" "$forwarder" 2>/dev/null
exit 1
