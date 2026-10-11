#!/bin/sh
# Runs a command with a temporary PostgreSQL in Docker and removes it with its volume
# afterwards. The command receives JUNTO_TEST_DATABASE_URL.
#
# The container is named by JUNTO_TEST_POSTGRES_CONTAINER (default junto-dev-postgres). Two
# checkouts on one machine run their tests at the same time when each sets another name.
set -eu

if [ "$#" -eq 0 ]; then
    echo "usage: $0 <command> [arguments]" >&2
    exit 2
fi

name="${JUNTO_TEST_POSTGRES_CONTAINER:-junto-dev-postgres}"
image="${JUNTO_TEST_POSTGRES_IMAGE:-postgres:17-alpine}"

remove() {
    docker rm -f -v "$name" >/dev/null 2>&1 || true
}

remove
trap remove EXIT
trap 'exit 130' INT TERM

docker run -d --name "$name" \
    -e POSTGRES_HOST_AUTH_METHOD=trust \
    -p 127.0.0.1::5432 \
    "$image" -c max_connections=400 -c fsync=off >/dev/null

port=$(docker port "$name" 5432/tcp | head -n 1 | sed 's/.*://')

attempts=0
until docker exec "$name" pg_isready -q -h 127.0.0.1 -U postgres; do
    attempts=$((attempts + 1))
    if [ "$attempts" -ge 120 ]; then
        echo "PostgreSQL did not become ready" >&2
        exit 1
    fi
    sleep 0.5
done

export JUNTO_TEST_DATABASE_URL="postgres://postgres@127.0.0.1:${port}/postgres"
"$@"
