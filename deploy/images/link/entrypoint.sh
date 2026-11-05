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
#   LINK_NAME              the name of this end in log lines and notices, such as "data host"
#   LINK_RESTART_AFTER     seconds without a handshake after which the process ends, so that
#                          Docker starts the link again; at least 180 (ADR 0003)
#   LINK_NOTICE_URL        optional; receives an HTTP POST with a plain text body when the
#                          link goes down and when it is up again
#
# /keys/down records a link that is down across these restarts: the time the link went down,
# the number of restarts, and whether the notice of the loss was delivered.
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
for name in LINK_ADDRESS LINK_PEER_ADDRESS LINK_NAME LINK_RESTART_AFTER; do
    eval "value=\${$name:-}"
    [ -n "$value" ] || { echo "link: $name is not set" >&2; exit 2; }
done
# WireGuard rejects the keys of a session 180 seconds after its handshake, and renews them
# about every 2 minutes while packets pass. A shorter bound would end a working link.
case "$LINK_RESTART_AFTER" in
    *[!0-9]*) echo "link: LINK_RESTART_AFTER is not a number of seconds: $LINK_RESTART_AFTER" >&2; exit 2 ;;
esac
if [ "$LINK_RESTART_AFTER" -lt 180 ]; then
    echo "link: LINK_RESTART_AFTER is $LINK_RESTART_AFTER; it must be at least 180 seconds" >&2
    exit 2
fi
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

notice() {
    [ -n "${LINK_NOTICE_URL:-}" ] || return 0
    if wget -q -T 10 -O /dev/null --header "Content-Type: text/plain" --post-data "$1" "$LINK_NOTICE_URL"; then
        echo "link: notice delivered: $1"
    else
        echo "link: the notice was not delivered to LINK_NOTICE_URL: $1" >&2
        return 1
    fi
}
utc() {
    date -u -d "@$1" '+%Y-%m-%d %H:%M:%S UTC'
}
down_since=""
down_restarts=0
down_notified=1
if [ -s /keys/down ]; then
    read -r down_since down_restarts down_notified < /keys/down || true
fi
case "$down_since$down_restarts$down_notified" in
    *[!0-9]*) down_since="" down_restarts=0 down_notified=1 ;;
esac
save_down() {
    printf '%s %s %s\n' "$down_since" "$down_restarts" "$down_notified" > /keys/down.new
    mv /keys/down.new /keys/down
}
down_text() {
    echo "Junto link of the $LINK_NAME is down since $(utc "$down_since"): no WireGuard handshake with the other host for $LINK_RESTART_AFTER seconds. The link restarts every $LINK_RESTART_AFTER seconds until the other host answers."
}

started=$(date +%s)
next_notice=$started
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
    now=$(date +%s)
    handshake=$(wg show wg0 latest-handshakes 2>/dev/null | awk '{ print $2; exit }')
    case "$handshake" in ''|*[!0-9]*) handshake=0 ;; esac
    if [ -n "$down_since" ] && [ "$handshake" -gt 0 ] && [ "$now" -ge "$next_notice" ]; then
        if notice "Junto link of the $LINK_NAME is up again since $(utc "$handshake"). It was down since $(utc "$down_since"). Restarts while it was down: $down_restarts."; then
            rm -f /keys/down
            down_since=""
        else
            next_notice=$((now + 60))
        fi
    elif [ -n "$down_since" ] && [ "$down_notified" -eq 0 ] && [ "$now" -ge "$next_notice" ]; then
        if notice "$(down_text)"; then
            down_notified=1
            save_down
        else
            next_notice=$((now + 60))
        fi
    fi
    reference=$started
    [ "$handshake" -le "$started" ] || reference=$handshake
    if [ "$((now - reference))" -ge "$LINK_RESTART_AFTER" ]; then
        echo "link: no handshake with the other host since $(utc "$reference"); the process ends so that Docker starts the link again" >&2
        if [ -z "$down_since" ]; then
            down_since=$now
            down_restarts=0
            down_notified=0
        fi
        down_restarts=$((down_restarts + 1))
        if [ "$down_notified" -eq 0 ] && notice "$(down_text)"; then
            down_notified=1
        fi
        save_down
        # shellcheck disable=SC2086
        kill $tunnel $forwarders 2>/dev/null
        exit 1
    fi
    sleep 1
done
