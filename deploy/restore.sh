#!/bin/sh
# Restores a backup that deploy/backup.sh made (docs/install.md, "Restore"). It replaces the
# database, the attachments and the recordings of the deployment. Put the file "env" of the
# backup in place as deploy/.env first: the database of the backup has the password of that
# file.
#
#   deploy/restore.sh <form file> <directory of the backup>
#
# The server, the recorder and the worker are stopped and stay stopped. Start the deployment
# afterwards with "docker compose -f <form file> up -d".
set -eu

if [ "$#" -ne 2 ]; then
    echo "usage: $0 <form file> <directory of the backup>" >&2
    exit 2
fi
deploy=$(cd "$(dirname "$0")" && pwd)
form=$(basename "$1")
settings=${JUNTO_ENV_FILE:-$deploy/.env}
backup=$(cd "$2" && pwd)
cd "$deploy"

if [ ! -f "$form" ] || [ ! -f "$settings" ]; then
    echo "restore: $deploy/$form or $settings does not exist" >&2
    exit 2
fi
compose() {
    docker compose --env-file "$settings" -f "$form" "$@"
}
verify() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum -c SHA256SUMS
    else
        shasum -a 256 -c SHA256SUMS
    fi
}

for file in database.dump files.tar env SHA256SUMS; do
    if [ ! -f "$backup/$file" ]; then
        echo "restore: $backup/$file does not exist" >&2
        exit 2
    fi
done
if ! (cd "$backup" && verify > /dev/null); then
    echo "restore: a file of the backup differs from its checksum" >&2
    exit 1
fi

# The archive of deploy/backup.sh holds the two directories and nothing else. An archive
# with another member name is refused: such a member would be unpacked outside the staging
# directories.
if ! names=$(tar -tf "$backup/files.tar"); then
    echo "restore: $backup/files.tar is not a readable archive" >&2
    exit 1
fi
if printf '%s\n' "$names" | grep -v -E '^(attachments|recordings)(/.*)?$' | grep -q . \
    || printf '%s\n' "$names" | grep -q -E '(^|/)\.\.(/|$)'; then
    echo "restore: $backup/files.tar has a member outside attachments/ and recordings/" >&2
    exit 1
fi
# The database of the backup gets the password of the settings file, whatever password the
# database of this host had.
password=$(sed -n 's/^JUNTO_POSTGRES_PASSWORD=//p' "$settings" | head -n 1)
case "$password" in
    "" | *[!A-Za-z0-9]*)
        echo "restore: JUNTO_POSTGRES_PASSWORD of $settings is empty or has other characters than letters and digits" >&2
        exit 2
        ;;
esac

compose stop server worker recorder
compose up -d --wait postgres

sql() {
    compose exec -T postgres psql -U junto -d postgres -v ON_ERROR_STOP=1 -q
}
remove_staging() {
    compose run --rm --no-deps -T --entrypoint sh server -c \
        'rm -rf /var/lib/junto/attachments/.restore /var/lib/junto/recordings/.restore' || true
    compose exec -T postgres dropdb -U junto --maintenance-db=postgres --if-exists --force junto_restore || true
}
staged=""
give_up() {
    code=$?
    if [ "$code" -ne 0 ] && [ -z "$staged" ]; then
        remove_staging
        echo "restore: failed before anything was replaced. The database and the files are as they were." >&2
    elif [ "$code" -ne 0 ]; then
        echo "restore: failed while the staged copy replaced the data. Run the restore again." >&2
    fi
    exit "$code"
}
trap give_up EXIT
trap 'exit 130' INT TERM

# Step 1: the dump is loaded into a new database, and the archive is unpacked into a staging
# directory of each volume. Nothing of the deployment is replaced yet.
remove_staging
compose exec -T postgres createdb -U junto --maintenance-db=postgres --owner=junto junto_restore
compose exec -T postgres pg_restore -U junto -d junto_restore --no-owner --exit-on-error < "$backup/database.dump"
compose run --rm --no-deps -T --entrypoint sh server -c '
    set -eu
    cd /var/lib/junto
    mkdir attachments/.restore recordings/.restore
    # The names of the files and of hard links get the staging directory, not the targets
    # of symbolic links.
    tar --numeric-owner -xpf - \
        --transform "s,^attachments\(/\|\$\),attachments/.restore\1,rSh" \
        --transform "s,^recordings\(/\|\$\),recordings/.restore\1,rSh"
' < "$backup/files.tar"

# Step 2: the staged copies take the places of the data. These are renames and deletions of
# the old data; they need no further disk space.
staged=1
compose run --rm --no-deps -T --entrypoint sh server -c '
    set -eu
    for volume in /var/lib/junto/attachments /var/lib/junto/recordings; do
        find "$volume" -mindepth 1 -maxdepth 1 ! -name .restore -exec rm -rf {} +
        find "$volume/.restore" -mindepth 1 -maxdepth 1 -exec mv -t "$volume" {} +
        rmdir "$volume/.restore"
    done
'
compose exec -T postgres dropdb -U junto --maintenance-db=postgres --if-exists --force junto
echo 'alter database junto_restore rename to junto;' | sql
printf "alter user junto password '%s';\n" "$password" | sql

# The clients hold cursors of events that the restored database does not have (ADR 0010).
compose run --rm --no-deps -T server new-event-epoch

echo "restore: done. Start the deployment: docker compose -f $form up -d"
