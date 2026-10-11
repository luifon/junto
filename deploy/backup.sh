#!/bin/sh
# Makes a backup of a deployment: the database, the attachments, the recordings and the
# settings file (docs/install.md, "Backup"). The server is stopped first, so that no
# recording can start, and stays stopped while the copy is made, so that the database and
# the files match. A server that was stopped before stays stopped afterwards. Run it on the
# host that has the data: with one-host.yaml, two-hosts-data.yaml or local.yaml.
#
#   deploy/backup.sh <form file> <new directory for the backup>
#
# JUNTO_ENV_FILE names another settings file than deploy/.env. COMPOSE_PROJECT_NAME names
# another project than the one of the form file.
set -eu

if [ "$#" -ne 2 ]; then
    echo "usage: $0 <form file> <new directory for the backup>" >&2
    exit 2
fi
deploy=$(cd "$(dirname "$0")" && pwd)
form=$(basename "$1")
settings=${JUNTO_ENV_FILE:-$deploy/.env}
destination=$2
cd "$deploy"

if [ ! -f "$form" ] || [ ! -f "$settings" ]; then
    echo "backup: $deploy/$form or $settings does not exist" >&2
    exit 2
fi
compose() {
    docker compose --env-file "$settings" -f "$form" "$@"
}
checksums() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$@"
    else
        shasum -a 256 "$@"
    fi
}

umask 077
if ! mkdir "$destination"; then
    echo "backup: $destination exists already or cannot be created" >&2
    exit 1
fi
destination=$(cd "$destination" && pwd)

stopped=""
for service in server worker; do
    if [ "$(compose ps --status running --format '{{.Service}}' "$service" 2>/dev/null)" = "$service" ]; then
        stopped="$stopped $service"
    fi
done
start_again() {
    code=$?
    if [ "$code" -ne 0 ]; then
        rm -f "$destination/database.dump" "$destination/files.tar" "$destination/env" "$destination/SHA256SUMS"
        rmdir "$destination" 2>/dev/null || true
        echo "backup: no backup was written." >&2
    fi
    if [ -n "$stopped" ]; then
        # shellcheck disable=SC2086
        compose start $stopped || echo "backup: start these services by hand:$stopped" >&2
    fi
    exit "$code"
}
trap start_again EXIT
trap 'exit 130' INT TERM
if [ -n "$stopped" ]; then
    # shellcheck disable=SC2086
    compose stop $stopped
fi

# The server is stopped, so no recording starts from here on. A recording that runs now
# still writes files, and the backup does not include a part of a recording (ADR 0005).
status=0
compose run --rm --no-deps -T server check-upgrade || status=$?
if [ "$status" -eq 3 ]; then
    echo "backup: a recording runs. End it, then start the backup again." >&2
    exit 3
elif [ "$status" -ne 0 ]; then
    echo "backup: the state of the recordings could not be read" >&2
    exit 1
fi

compose exec -T postgres pg_dump -U junto -d junto --format=custom > "$destination/database.dump"
compose run --rm --no-deps -T --entrypoint tar server \
    --numeric-owner -C /var/lib/junto -cf - attachments recordings > "$destination/files.tar"
cp "$settings" "$destination/env"
(cd "$destination" && checksums database.dump files.tar env > SHA256SUMS)

echo "backup: written to $destination"
echo "backup: the file env holds the secrets of the deployment. Store the directory accordingly."
