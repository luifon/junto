#!/bin/sh
# One end of the private link of the two-host form (ADR 0003): WireGuard in userspace and a
# list of TCP forwarders. The container forwards no IP packets.
#
#   genkey   create /keys/private once and print the public key
#   run      bring the link up and forward TCP (default)
#
# Environment of "run":
#   LINK_ADDRESS           the address of this end on the link, without a prefix length
#   LINK_PEER_ADDRESS      the address of the other end on the link
#   LINK_PEER_PUBLIC_KEY   the public key of the other end
#   LINK_PEER_PUBLIC_KEY_SETTING  the name of the setting of that key, for the message
#   LINK_LISTEN_PORT       UDP port of the end that receives the connection (public host)
#   LINK_ENDPOINT          host:port of the other end, for the end that opens the connection
#   LINK_KEEPALIVE         seconds between keepalive packets, for the end that opens it
#   LINK_FORWARDS          forwarders separated by spaces, each
#                          <listen address>:<listen port>=<target host>:<target port>
set -eu
umask 077

case "${1:-run}" in
    genkey)
        [ -s /keys/private ] || wg genkey > /keys/private
        wg pubkey < /keys/private
        exit 0
        ;;
    run) ;;
    *)
        echo "link: unknown command $1" >&2
        exit 2
        ;;
esac

if [ -z "${LINK_PEER_PUBLIC_KEY:-}" ]; then
    echo "link: the public key of the other host is not set. Run \"link genkey\" on the other host and set ${LINK_PEER_PUBLIC_KEY_SETTING:-LINK_PEER_PUBLIC_KEY} in deploy/.env to the key that it prints." >&2
    exit 2
fi
for name in LINK_ADDRESS LINK_PEER_ADDRESS; do
    eval "value=\${$name:-}"
    [ -n "$value" ] || { echo "link: $name is not set" >&2; exit 2; }
done
if [ ! -s /keys/private ]; then
    echo "link: /keys/private does not exist; run the command genkey first" >&2
    exit 2
fi

wireguard-go -f wg0 &
tunnel=$!
# The configuration tool is called only after the control socket of the userspace process
# exists. A call before that makes the kernel load its own WireGuard module (ADR 0003).
tries=0
until [ -S /var/run/wireguard/wg0.sock ]; do
    tries=$((tries + 1))
    if [ "$tries" -ge 100 ]; then
        echo "link: the control socket of wg0 did not appear" >&2
        exit 1
    fi
    sleep 0.1
done

set -- set wg0 private-key /keys/private
[ -z "${LINK_LISTEN_PORT:-}" ] || set -- "$@" listen-port "$LINK_LISTEN_PORT"
set -- "$@" peer "$LINK_PEER_PUBLIC_KEY" allowed-ips "$LINK_PEER_ADDRESS/32"
[ -z "${LINK_ENDPOINT:-}" ] || set -- "$@" endpoint "$LINK_ENDPOINT"
[ -z "${LINK_KEEPALIVE:-}" ] || set -- "$@" persistent-keepalive "$LINK_KEEPALIVE"
wg "$@"

ip address add "$LINK_ADDRESS/32" peer "$LINK_PEER_ADDRESS/32" dev wg0
ip link set wg0 mtu 1420 up

forwarders=""
for forward in ${LINK_FORWARDS:-}; do
    listen=${forward%%=*}
    target=${forward#*=}
    listen_address=${listen%:*}
    listen_port=${listen##*:}
    if [ "$listen" = "$forward" ] || [ -z "$listen_address" ] || [ -z "$listen_port" ] || [ -z "$target" ]; then
        echo "link: \"$forward\" is not <listen address>:<listen port>=<target host>:<target port>" >&2
        kill "$tunnel" 2>/dev/null
        exit 2
    fi
    socat "TCP-LISTEN:$listen_port,bind=$listen_address,fork,reuseaddr,keepalive,nodelay,su=nobody" \
        "TCP:$target,connect-timeout=5,keepalive,nodelay" &
    forwarders="$forwarders $!"
done

# shellcheck disable=SC2086
trap 'kill $tunnel $forwarders 2>/dev/null; exit 0' TERM INT
while :; do
    for process in $tunnel $forwarders; do
        if ! kill -0 "$process" 2>/dev/null; then
            echo "link: a process of the link ended" >&2
            # shellcheck disable=SC2086
            kill $tunnel $forwarders 2>/dev/null
            exit 1
        fi
    done
    sleep 1
done
