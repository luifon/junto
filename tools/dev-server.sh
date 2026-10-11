#!/bin/sh
# Starts a Junto server for development on this machine and prints what a person needs to
# join it: the address, and the one-time owner code that the server prints at its start.
# The database is a temporary PostgreSQL in Docker, and the files of messages go to a
# temporary directory; both are removed with the server when the script ends (Control-C).
#
# usage: tools/dev-server.sh [--port <port>] [--name <name>] [--media]
#
#   --port <port>   the port of the server on 127.0.0.1 (default 8080)
#   --name <name>   the name that the server shows (default "Local server")
#   --media         also start a LiveKit server in Docker, so that calls have a media server
#
# Two servers on one machine: start the second with another port and with another value of
# JUNTO_TEST_POSTGRES_CONTAINER.
set -eu

here=$(cd "$(dirname "$0")/.." && pwd)
port=8080
name="Local server"
media=0

usage() {
    sed -n '2,13p' "$0" | sed 's/^# \{0,1\}//' >&2
    exit 2
}

expect=""
for argument in "$@"; do
    case "$expect" in
        port) port=$argument; expect=""; continue ;;
        name) name=$argument; expect=""; continue ;;
    esac
    case "$argument" in
        --port) expect=port ;;
        --name) expect=name ;;
        --media) media=1 ;;
        *) usage ;;
    esac
done
[ -z "$expect" ] || usage

# The first run starts the containers and runs this script again inside them.
if [ -z "${JUNTO_TEST_DATABASE_URL:-}" ]; then
    export JUNTO_TEST_POSTGRES_CONTAINER="${JUNTO_TEST_POSTGRES_CONTAINER:-junto-dev-server-postgres}"
    if [ "$media" -eq 1 ]; then
        export JUNTO_TEST_LIVEKIT_CONTAINER="${JUNTO_TEST_LIVEKIT_CONTAINER:-junto-dev-server-livekit}"
        export JUNTO_TEST_WEBHOOK_ADDRESS="127.0.0.1:${port}"
        exec "$here/tools/with-test-postgres.sh" "$here/tools/with-test-livekit.sh" "$0" "$@"
    fi
    exec "$here/tools/with-test-postgres.sh" "$0" "$@"
fi

cd "$here"
tools/serial.sh cargo build -p junto-server

export JUNTO_DATABASE_URL="$JUNTO_TEST_DATABASE_URL"
export JUNTO_LISTEN_ADDRESS="127.0.0.1:${port}"
export JUNTO_SERVER_NAME="$name"
export JUNTO_LOG_FORMAT="${JUNTO_LOG_FORMAT:-text}"
export JUNTO_LOG_LEVEL="${JUNTO_LOG_LEVEL:-warn}"
# The files of messages: a directory of this run (ADR 0408).
attachments=$(mktemp -d "${TMPDIR:-/tmp}/junto-dev-attachments.XXXXXX")
attachments=$(cd "$attachments" && pwd -P)
trap 'rm -rf "$attachments"' EXIT
trap 'exit 130' INT TERM
export JUNTO_ATTACHMENTS_DIR="$attachments"
if [ "$media" -eq 1 ]; then
    export JUNTO_LIVEKIT_API_URL="$JUNTO_TEST_LIVEKIT_API_URL"
    export JUNTO_LIVEKIT_URL="$JUNTO_TEST_LIVEKIT_URL"
    export JUNTO_LIVEKIT_API_KEY="$JUNTO_TEST_LIVEKIT_API_KEY"
    export JUNTO_LIVEKIT_API_SECRET="$JUNTO_TEST_LIVEKIT_API_SECRET"
fi

cat <<TEXT

Junto server "${name}"

    Address:     http://127.0.0.1:${port}
    Owner code:  printed below by the server

Type the address into "Add server" of the application, then the owner code into
"I have a code". The database and the files of messages are temporary. Control-C ends the
server and removes them.

TEXT

# Not `exec`: the scripts around this one remove the containers when the server has ended.
target/debug/junto-server
