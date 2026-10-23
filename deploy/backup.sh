#!/bin/sh
# Makes a backup of a deployment while the server runs: the database, the attachments, the
# recordings and the settings file (docs/install.md, "Backup"; ADR 0018, ADR 0021). The
# server holds every deletion of a file back from the first step to the last, so the files
# of the archive are the ones that the copy of the database names. Run it on the host that
# has the data: with one-host.yaml, two-hosts-data.yaml or local.yaml.
#
#   deploy/backup.sh <form file> <new directory for the backup>
#
# JUNTO_ENV_FILE names another settings file than deploy/.env. COMPOSE_PROJECT_NAME names
# another project than the one of the form file.
#
# Before the backup begins, the server removes the directories that no row of the database
# names and that were last changed more than 24 hours ago (ADR 0018).
#
# Exit codes: 0 for a backup; 4 for a backup that was written without the directory of a
# complete attachment or of a finished recording, which the file left-out names; every
# other code: no backup.
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
runs() {
    [ "$(compose ps --status running --format '{{.Service}}' "$1" 2>/dev/null)" = "$1" ]
}
# Runs a program with the settings and the volumes of the server: in the container of the
# server while it runs, otherwise in a container of its own. A second container cannot
# start beside a running server in the forms that give the server a fixed address.
in_server() {
    if runs server; then
        compose exec -T server "$@"
    else
        program=$1
        shift
        compose run --rm --no-deps -T --entrypoint "$program" server "$@"
    fi
}
checksums() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$@"
    else
        shasum -a 256 "$@"
    fi
}

if ! runs postgres; then
    echo "backup: the container postgres does not run. Start it: docker compose -f $form up -d postgres" >&2
    exit 1
fi

# The scripts of this release work with the role of the server (ADR 0022).
if [ "$(compose exec -T postgres psql -U junto -d postgres -At -c "select count(*) from pg_roles where rolname = 'junto_server'" < /dev/null)" != 1 ]; then
    echo "backup: the database server has no role junto_server. An earlier release created this deployment: run deploy/database-user.sh $form first (docs/install.md, \"The database user of an earlier release\")." >&2
    exit 1
fi

umask 077
if ! mkdir "$destination"; then
    echo "backup: $destination exists already or cannot be created" >&2
    exit 1
fi
destination=$(cd "$destination" && pwd)

scratch="$destination/.scratch"
mkdir "$scratch"
backup=""
written=""
# A signal can end the script during a command whose standard error goes to a file of the
# scratch directory, and the shell runs the trap with that redirection. The trap writes to
# the standard error of the script, which file descriptor 3 keeps.
exec 3>&2
ended() {
    code=$?
    exec 2>&3
    if [ -n "$backup" ]; then
        in_server junto-server backup-end "$backup" < /dev/null > /dev/null \
            || echo "backup: the server still holds deletions back, for 24 hours at most. End that with: junto-server backup-end $backup" >&2
    fi
    rm -rf "$scratch"
    if [ -z "$written" ]; then
        [ "$code" -ne 0 ] || code=1
        rm -f "$destination/database.dump" "$destination/files.tar" "$destination/env" \
            "$destination/left-out" "$destination/schema" "$destination/SHA256SUMS"
        rmdir "$destination" 2>/dev/null || true
        echo "backup: no backup was written." >&2
    fi
    exit "$code"
}
trap ended EXIT
trap 'exit 130' INT TERM

# The newest migration of the server binary, which the backup states in its file "schema":
# a restore compares it with its own binary before it stops a container (ADR 0018). It must
# be the schema of the database. It is not when the server is stopped and its image was
# built from another release than the one that ran; the commands of that binary are then
# written for another schema, so no backup is made.
in_server junto-server schema-version < /dev/null > "$destination/schema" \
    || { echo "backup: the server binary does not state its schema. This script needs the server of its own release." >&2; exit 1; }
if [ "$(wc -l < "$destination/schema" | tr -d ' ')" != 1 ] || ! grep -q -x -E 'schema [0-9]+' "$destination/schema"; then
    echo "backup: the server binary printed another answer than \"schema <number>\"" >&2
    exit 1
fi
of_database=$(compose exec -T postgres psql -U junto_server -d junto -At -c 'select max(version) from _sqlx_migrations where success' < /dev/null)
if [ "schema $of_database" != "$(cat "$destination/schema")" ]; then
    echo "backup: the image of the server has the $(cat "$destination/schema"), and the database has the schema $of_database. A backup needs the server of the release that the database has: build the image in the checkout of that release (docker compose -f $form build server)." >&2
    exit 1
fi

# A restore and the way back from an upgrade can leave directories that no row names. The
# server removes the ones that were last changed more than 24 hours ago; it cannot tell a
# younger one from a directory that it writes. It removes nothing while a backup holds
# deletions back, so this runs before the backup begins. A failure does not stop the backup.
if ! in_server junto-server unnamed-files --remove < /dev/null > "$scratch/unnamed" 2> "$scratch/unnamed-error"; then
    echo "backup: the server did not remove every directory that the database does not name. The backup goes on. The server stated:" >&2
    cat "$scratch/unnamed-error" >&2
fi
removed=$(grep -c '^removed ' "$scratch/unnamed" || true)

# Step 1: from here on the server deletes no directory of an attachment or of a recording.
begun=$(in_server junto-server backup-begin < /dev/null)
backup=$(printf '%s\n' "$begun" | sed -n 's/^backup \([0-9a-f-]\{36\}\) .*$/\1/p')
if [ -z "$backup" ]; then
    echo "backup: the server did not state the identifier of a backup" >&2
    exit 1
fi

# Step 2: the copy of the database. pg_dump reads one state of it.
compose exec -T postgres pg_dump -U junto_server -d junto --format=custom < /dev/null > "$destination/database.dump"

# Step 3: the directories that the copy of the database names. The list is made after the
# copy. "file" lines are copied; "skip" lines are directories that are still written.
in_server junto-server backup-files "$backup" < /dev/null > "$scratch/listed"
if grep -v -E '^(file (attachments|recordings)/[A-Za-z0-9_/-]+|skip [A-Za-z0-9_/-]+ [a-z_]+)$' "$scratch/listed" | grep -q .; then
    echo "backup: the server printed a line of the list that this script does not know" >&2
    exit 1
fi
# A listed directory that is absent is a lost file: the list names an attachment only while
# it is complete or its directory exists, and a recording only when it has files.
sed -n 's/^file //p' "$scratch/listed" > "$scratch/named"
in_server bash -c '
    cd /var/lib/junto || exit 1
    while IFS= read -r directory; do
        if [ -d "$directory" ]; then
            printf "present %s\n" "$directory"
        else
            printf "absent %s\n" "$directory"
        fi
    done
' < "$scratch/named" > "$scratch/checked"
if [ "$(wc -l < "$scratch/checked")" -ne "$(wc -l < "$scratch/named")" ]; then
    echo "backup: the listed directories could not all be looked at" >&2
    exit 1
fi
{
    grep '^skip ' "$scratch/listed" || true
    grep '^absent ' "$scratch/checked" || true
} > "$destination/left-out"
sed -n 's/^present //p' "$scratch/checked" > "$scratch/present"

# tar ends with the code 1 when a file changed or went away while it was read. The server
# writes the parts of a running upload and the directories of a transcription run beside
# complete files, so that code is accepted and the lines of tar are shown. Every other
# code fails the backup. The two directories themselves are members of every archive.
in_server bash -c '
    cd /var/lib/junto || exit 2
    code=0
    tar --numeric-owner -cf - --no-recursion attachments recordings \
        --recursion --verbatim-files-from --files-from=/dev/stdin || code=$?
    [ "$code" -le 1 ] || exit "$code"
' < "$scratch/present" > "$destination/files.tar"
# The archive is complete when the command ended without a failure, every directory that
# was given to tar is a member, and the archive ends as tar ends one, with blocks of zero
# bytes.
tar -tf "$destination/files.tar" > "$scratch/listing"
sed 's,/$,,' "$scratch/listing" | LC_ALL=C sort -u > "$scratch/members"
LC_ALL=C sort -u "$scratch/present" > "$scratch/expected"
if [ -n "$(LC_ALL=C comm -23 "$scratch/expected" "$scratch/members" | head -n 1)" ] \
    || [ "$(wc -c < "$destination/files.tar")" -lt 1024 ] \
    || [ -n "$(tail -c 1024 "$destination/files.tar" | tr -d '\000' | head -c 1)" ]; then
    echo "backup: the archive of the files is not complete" >&2
    exit 1
fi
# The server writes files and directories only, and a restore refuses an archive with a
# link.
tar -tvf "$destination/files.tar" > "$scratch/listing"
if grep -q '^[^-d]' "$scratch/listing"; then
    echo "backup: the directories of the attachments or of the recordings hold something that is not a file or a directory. Remove it, then start the backup again:" >&2
    grep '^[^-d]' "$scratch/listing" | head -n 5 >&2
    exit 1
fi

# Step 4: the server deletes files again. The command fails for a backup that expired: a
# backup ends by itself after 24 hours, and the server then held deletions back for a part
# of the copy only.
if ! in_server junto-server backup-end "$backup" < /dev/null > /dev/null 2> "$scratch/end-error"; then
    cat "$scratch/end-error" >&2
    if grep -q ' expired at ' "$scratch/end-error"; then
        backup=""
        echo "backup: the backup expired before its files were copied. The server may have deleted files during the copy." >&2
    fi
    exit 1
fi
backup=""

cp "$settings" "$destination/env"
skipped=$(grep -c '^skip ' "$destination/left-out" || true)
lost=$(grep -c '^absent ' "$destination/left-out" || true)
(cd "$destination" && checksums database.dump files.tar env left-out schema > SHA256SUMS)
written=1

echo "backup: written to $destination"
if [ "$removed" -gt 0 ]; then
    echo "backup: before the backup began, the server removed $removed directories that no row of the database names:"
    grep '^removed ' "$scratch/unnamed"
fi
if [ "$skipped" -gt 0 ]; then
    echo "backup: $skipped directories are not in the backup, because an upload or a recording had not ended. The file left-out names them."
fi
echo "backup: the file env holds the secrets of the deployment. Store the directory accordingly."
if [ "$lost" -gt 0 ]; then
    echo "backup: $lost directories of complete attachments or of finished recordings that the database names are not on the disk. The file left-out names them. The backup was written without them; this ends with the code 4." >&2
    exit 4
fi
