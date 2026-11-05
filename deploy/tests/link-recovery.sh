#!/bin/sh
# Starts only the link containers of the two files of the two-host form (ADR 0003) on one
# machine, as two Compose projects with separate networks, and a receiver of notices. Checks
# that each end of the link starts again by itself when the tunnel passes no packets, and that
# each end sends one notice when it goes down and one when it is up again:
# 1. The link of the public host is stopped. The link of the data host ends after
#    JUNTO_LINK_RESTART_AFTER seconds without a handshake, Docker starts it again, it ends
#    again, and it sends one notice in all. When the public host returns, it sends the notice
#    that it is up.
# 2. The link of the public host drops every packet to the data host (a route in its own
#    network namespace). Its link ends after its bound, Docker starts it again without the
#    route, and it sends its two notices.
# Before that, both ends stay up for longer than both bounds without a start. The receiver
# refuses the first notice, which must arrive later once, and every notice is plain text.
# A bound below 180 seconds is refused. The run takes about 20 minutes. The networks are
# 172.31.250.0/24 to 172.31.252.0/24, and the UDP port of the link is
# JUNTO_TEST_LINK_PORT (default 51821). Everything is removed at the end.
#
#   deploy/tests/link-recovery.sh [<name prefix of the two projects>]
set -eu
prefix=${1:-junto-link-recovery}
port=${JUNTO_TEST_LINK_PORT:-51821}
deploy=$(cd "$(dirname "$0")/.." && pwd)
work=$(mktemp -d "${TMPDIR:-/tmp}/$prefix.XXXXXX")
tools_image=busybox:1.37.0@sha256:bdf57e528e45e4433820e045b29b4597825a1c9e38353532d90a01445013f82e
receiver=$prefix-receiver
data_limit=180
public_limit=240
cd "$deploy"

public() {
    docker compose -p "$prefix-public" --env-file "$work/public.env" -f two-hosts-public.yaml "$@"
}
data() {
    docker compose -p "$prefix-data" --env-file "$work/data.env" -f two-hosts-data.yaml "$@"
}
remove() {
    code=$?
    if [ "$code" -ne 0 ]; then
        public ps -a || true
        data ps -a || true
        public logs --tail 40 link || true
        data logs --tail 40 link || true
        docker exec "$receiver" cat /notices 2>/dev/null || true
    fi
    docker rm -f "$receiver" > /dev/null 2>&1 || true
    data down -v --timeout 20 > /dev/null 2>&1 || true
    public down -v --timeout 20 > /dev/null 2>&1 || true
    rm -rf "$work"
    exit "$code"
}
trap remove EXIT
trap 'exit 130' INT TERM

mkdir "$work/models"
(
    umask 077
    ./generate-secrets.sh > "$work/secrets"
    {
        grep -v JUNTO_POSTGRES_PASSWORD "$work/secrets"
        echo "JUNTO_SERVER_HOST=chat.example.test"
        echo "JUNTO_LIVEKIT_HOST=livekit.example.test"
        echo "JUNTO_TURN_HOST=turn.example.test"
        echo "JUNTO_ACME_EMAIL=admin@example.test"
        echo "JUNTO_PUBLIC_IP=172.31.250.1"
        echo "JUNTO_BACKEND_SUBNET=172.31.250.0/24"
        echo "JUNTO_BACKEND_GATEWAY=172.31.250.1"
        echo "JUNTO_BACKEND_REDIS=172.31.250.11"
        echo "JUNTO_BACKEND_LINK=172.31.250.12"
        echo "JUNTO_INBOUND_SUBNET=172.31.252.0/24"
        echo "JUNTO_INBOUND_GATEWAY=172.31.252.1"
        echo "JUNTO_INBOUND_LINK=172.31.252.12"
        echo "JUNTO_LINK_PORT=$port"
        echo "JUNTO_LINK_MEMORY=64m"
        echo "JUNTO_LINK_RESTART_AFTER=$public_limit"
        echo "JUNTO_LINK_NOTICE_URL=http://receiver:8080/cgi-bin/notice"
    } > "$work/public.env"
    {
        cat "$work/secrets"
        echo "JUNTO_SERVER_HOST=chat.example.test"
        echo "JUNTO_LIVEKIT_HOST=livekit.example.test"
        echo "JUNTO_MODELS_DIR=$work/models"
        # The "public host" is reached at the address of the machine in its own network.
        echo "JUNTO_PUBLIC_IP=172.31.250.1"
        echo "JUNTO_LINK_PORT=$port"
        echo "JUNTO_BACKEND_SUBNET=172.31.251.0/24"
        echo "JUNTO_BACKEND_GATEWAY=172.31.251.1"
        echo "JUNTO_BACKEND_SERVER=172.31.251.10"
        echo "JUNTO_BACKEND_REDIS=172.31.251.11"
        echo "JUNTO_BACKEND_LINK=172.31.251.12"
        echo "JUNTO_LINK_MEMORY=64m"
        echo "JUNTO_LINK_RESTART_AFTER=$data_limit"
        echo "JUNTO_LINK_NOTICE_URL=http://receiver:8080/cgi-bin/notice"
    } > "$work/data.env"
)
public build link
data build link
public_key=$(public run --rm --no-deps -T link genkey)
data_key=$(data run --rm --no-deps -T link genkey)
(
    umask 077
    echo "JUNTO_LINK_DATA_PUBLIC_KEY=$data_key" >> "$work/public.env"
    echo "JUNTO_LINK_PUBLIC_PUBLIC_KEY=$public_key" >> "$work/data.env"
)

if output=$(data run --rm --no-deps -T -e LINK_RESTART_AFTER=179 link 2>&1); then
    echo "FAIL  the link starts with a bound of 179 seconds"
    exit 1
fi
case "$output" in
    *"at least 180 seconds"*) echo "ok    a bound below 180 seconds is refused" ;;
    *) echo "FAIL  the link refuses a bound of 179 seconds without naming the minimum: $output"; exit 1 ;;
esac

public up -d --no-deps link
data up -d --no-deps --wait --wait-timeout 120 link
public up -d --no-deps --wait --wait-timeout 60 link
echo "ok    both ends of the link are healthy"

# The receiver refuses the first POST with 503, so that a notice must be sent again, and
# appends each later one to /notices as one line: its content type and its body.
mkdir "$work/cgi-bin"
cat > "$work/cgi-bin/notice" <<'EOF'
#!/bin/sh
body=$(head -c "$CONTENT_LENGTH")
posts=$(($(cat /posts 2>/dev/null || echo 0) + 1))
echo "$posts" > /posts
if [ "$posts" = 1 ]; then
    echo "$CONTENT_TYPE $body" >> /refused
    printf 'Status: 503 Service Unavailable\r\nContent-Type: text/plain\r\n\r\nbusy\n'
    exit 0
fi
echo "$CONTENT_TYPE $body" >> /notices
printf 'Content-Type: text/plain\r\n\r\nok\n'
EOF
chmod 755 "$work/cgi-bin/notice"
docker run -d --name "$receiver" --network "$prefix-data_backend" --network-alias receiver \
    --memory 32m -v "$work/cgi-bin:/www/cgi-bin:ro" "$tools_image" \
    sh -c 'touch /notices /refused && exec httpd -f -p 8080 -h /www' > /dev/null
docker network connect --alias receiver "$prefix-public_backend" "$receiver"

container() {
    "$1" ps -a -q link
}
restarts() {
    docker inspect --format '{{.RestartCount}}' "$(container "$1")"
}
# The number of accepted notices that hold a text. The test fails when the receiver cannot
# be read.
notices() {
    count=$(docker exec "$receiver" sh -c 'grep -c -F "$1" /notices; true' sh "$1") || count=""
    case "$count" in
        ''|*[!0-9]*) echo "FAIL  the receiver of the notices cannot be read" >&2; exit 1 ;;
    esac
    echo "$count"
}
# wait_for <seconds> <description> <command...>: runs the command every 2 seconds until it
# succeeds, and fails the test after the given time.
wait_for() {
    limit=$1 description=$2
    shift 2
    deadline=$(($(date +%s) + limit))
    until "$@"; do
        if [ "$(date +%s)" -ge "$deadline" ]; then
            echo "FAIL  $description: not within $limit seconds"
            exit 1
        fi
        sleep 2
    done
}
at_least() {
    value=$("$2" "$3") || exit 1
    [ "$value" -ge "$1" ]
}
expect_notices() {
    count=$(notices "$1") || exit 1
    if [ "$count" -ne "$2" ]; then
        echo "FAIL  $3: $count notices \"$1\", expected $2"
        exit 1
    fi
}

data_down="link of the data host is down"
data_up="link of the data host is up again"
public_down="link of the public host is down"
public_up="link of the public host is up again"

# Both ends that work stay up for longer than both bounds: the handshakes are renewed about
# every 2 minutes, and no end starts again.
started=$(date +%s)
while [ $(($(date +%s) - started)) -lt $((public_limit + 60)) ]; do sleep 10; done
if [ "$(restarts data)" != 0 ] || [ "$(restarts public)" != 0 ]; then
    echo "FAIL  a link that works started again: data $(restarts data), public $(restarts public)"
    exit 1
fi
expect_notices "Junto link" 0 "a link that works sends no notice"
echo "ok    both ends that work stay up for $((public_limit + 60)) seconds without a start and without a notice"

public stop link
wait_for $((data_limit * 2 + 120)) "the link of the data host starts again twice while the public host is stopped" \
    at_least 2 restarts data
expect_notices "$data_down" 1 "the data host stays down across two restarts"
refused=$(docker exec "$receiver" cat /refused)
case "$refused" in
    "text/plain Junto $data_down"*) ;;
    *) echo "FAIL  the first notice, which the receiver refused, is not the loss of the data host: $refused"; exit 1 ;;
esac
echo "ok    the notice that the receiver refused first arrived later once"
expect_notices "$data_up" 0 "the data host is not up while the public host is stopped"
echo "ok    the link of the data host starts again without a handshake, and sends one notice for the loss"

public start link
wait_for $((data_limit + 120)) "the data host sends the notice that its link is up" \
    at_least 1 notices "$data_up"
data up -d --no-deps --wait --wait-timeout 200 link
expect_notices "$public_down" 0 "the public host was stopped, not down"
echo "ok    the link of the data host is up again and healthy, and sends one notice for its return"

data_down_before=$(notices "$data_down")
public_restarts=$(restarts public)
endpoint=$(public exec -T link wg show wg0 endpoints | awk '{ print $2; exit }')
endpoint=${endpoint%:*}
case "$endpoint" in
    *.*.*.*) ;;
    *) echo "FAIL  the link of the public host has no endpoint of the data host: \"$endpoint\""; exit 1 ;;
esac
public exec -T link ip route add blackhole "$endpoint/32"
wait_for $((public_limit + 120)) "the link of the public host starts again when it receives no handshake" \
    at_least "$((public_restarts + 1))" restarts public
wait_for 200 "the public host sends the notice that its link is up" at_least 1 notices "$public_up"
expect_notices "$public_down" 1 "the public host sends one notice for its loss"
public up -d --no-deps --wait --wait-timeout 200 link
data up -d --no-deps --wait --wait-timeout 200 link
if [ "$(notices "$data_down")" -gt "$data_down_before" ]; then
    wait_for 200 "the data host sends the notice that its link is up after the loss on the public host" \
        at_least "$(notices "$data_down")" notices "$data_up"
fi
echo "ok    the link of the public host starts again without the route that dropped its packets, and sends its two notices"
if [ "$(docker exec "$receiver" grep -c -v '^text/plain ' /notices)" != 0 ]; then
    echo "FAIL  a notice is not plain text: $(docker exec "$receiver" cat /notices)"
    exit 1
fi
echo "ok    every notice is plain text"
echo "all checks passed"
