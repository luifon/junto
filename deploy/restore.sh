#!/bin/sh
# Restores a backup that deploy/backup.sh made (docs/install.md, "Restore"; ADR 0021). It
# replaces the database, the attachments and the recordings of the deployment, and deletes
# the exports, the transcription jobs of the spool, the partial files of the recorder and
# the copy of the database from before a migration. Put the file "env" of the backup in
# place as deploy/.env first: the role of the server gets the password of that file.
#
#   deploy/restore.sh <form file> <directory of the backup>
#
# A backup is restored with the release that wrote it: the script refuses a backup of
# another schema than the one of the server of this checkout, before it stops a container.
# JUNTO_SERVER_IMAGE and JUNTO_INIT_IMAGE name its prebuilt images. No image is built or pulled.
#
# The server, the recorder and the worker are stopped and stay stopped. Start the deployment
# afterwards with "docker compose -f <form file> up -d".
#
# While the data is replaced, the database "junto" does not exist, so a server does not
# start on data that is replaced in part. A run that was interrupted is followed by a
# second run, which restores everything again.
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
    docker compose --env-file "$settings" -f "$form" -f restore-images.yaml "$@"
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
# A backup of the first release with the backup commands has no file "schema".
listed="database.dump files.tar env"
if [ -f "$backup/schema" ]; then
    listed="$listed schema"
fi
for file in $listed; do
    if ! sed -n -E 's/^[0-9a-f]{64} [ *]//p' "$backup/SHA256SUMS" | grep -q -x -F "$file"; then
        echo "restore: $backup/SHA256SUMS has no checksum of $file" >&2
        exit 1
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
# The server writes files and directories only. A link could name a place outside the
# directories.
if ! kinds=$(tar -tvf "$backup/files.tar" | cut -c 1 | LC_ALL=C sort -u | tr -d '\n'); then
    echo "restore: $backup/files.tar is not a readable archive" >&2
    exit 1
fi
if [ -n "$(printf '%s' "$kinds" | tr -d 'd-')" ]; then
    echo "restore: $backup/files.tar has a member that is not a file or a directory" >&2
    exit 1
fi
# The container of PostgreSQL does not start with another password than letters and digits.
# Compose takes the last line of a setting that is there twice.
case "$(sed -n 's/^JUNTO_POSTGRES_PASSWORD=//p' "$settings" | tail -n 1)" in
    "" | *[!A-Za-z0-9]*)
        echo "restore: JUNTO_POSTGRES_PASSWORD of $settings is empty or has other characters than letters and digits" >&2
        exit 2
        ;;
esac

# PostgreSQL starts init as a dependency. The temporary server and recorder containers
# start without dependencies. Check all four services before even the schema container.
configuration=$(compose config)
service_image() {
    printf '%s\n' "$configuration" | sed -n "/^  $1:/,/^  [^ ]/s/^    image: //p" | tr -d "\"'"
}
image=$(service_image server)
for service in init postgres server recorder; do
    required=$(service_image "$service")
    if [ -z "$required" ]; then
        echo "restore: $form names no image for $service. Nothing was changed." >&2
        exit 1
    fi
    if ! docker image inspect "$required" > /dev/null 2>&1; then
        echo "restore: required image $required is not available locally. Nothing was changed." >&2
        if [ "$service" = server ] || [ "$service" = init ]; then
            echo "restore: load the saved release image: docker image load --input <release-images.tar>" >&2
            echo "restore: or build it explicitly in the checkout of the backup release: docker compose --env-file \"$settings\" -f $form build $service" >&2
        else
            echo "restore: provide it before restoring: docker pull $required" >&2
        fi
        exit 1
    fi
done

# The schema of the backup and the schema of the server image of this deployment
# (ADR 0018). The command "restored" accepts the schema of its own binary only, and a
# server does not start on a newer schema, so every other backup is refused here, before a
# container is stopped. The image is the one that the staging runs, which can be another
# than the image of a server that runs. Its container gets no network: a second container
# of the service cannot start beside a running server in the forms that give the server a
# fixed address. It is no container of a service, so this command names its limits and gives
# it a name of the project (ADR 0015).
if [ -f "$backup/schema" ]; then
    of_backup=$(sed -n 's/^schema \([0-9][0-9]*\)$/\1/p' "$backup/schema")
    if [ -z "$of_backup" ] || [ "$(wc -l < "$backup/schema" | tr -d ' ')" != 1 ]; then
        echo "restore: $backup/schema is not one line \"schema <number>\"" >&2
        exit 1
    fi
    named=$(printf '%s\n' "$configuration" | sed -n 's/^name: //p' | head -n 1 | tr -d '"')
    stated=$(docker run --rm --pull never --network none --name "${named:-junto}-server-schema-$$" \
        --memory 64m --cpus 0.5 --pids-limit 32 --read-only --cap-drop ALL \
        --security-opt no-new-privileges:true "$image" schema-version < /dev/null) || stated=""
    of_server=$(printf '%s\n' "$stated" | sed -n 's/^schema \([0-9][0-9]*\)$/\1/p')
    if [ -z "$of_server" ]; then
        echo "restore: the server image $image does not state its schema (junto-server schema-version). Build the image of this checkout: docker compose -f $form build server" >&2
        exit 1
    fi
    if [ "$of_backup" -gt "$of_server" ]; then
        echo "restore: the backup has the schema $of_backup, and the server of this checkout has the schema $of_server: a newer release wrote the backup. Nothing was changed. Check out the release that wrote the backup and restore with it." >&2
        exit 1
    fi
    if [ "$of_backup" -lt "$of_server" ]; then
        echo "restore: the backup has the schema $of_backup, and the server of this checkout has the schema $of_server: an earlier release wrote the backup. Nothing was changed. Check out the release that wrote the backup, restore with it, and upgrade afterwards." >&2
        exit 1
    fi
fi

# The comment that the server gives a copy of this database before a migration (ADR 0018).
copy_comment='junto: copy of the database junto before a migration'

compose stop server worker recorder
compose up -d --wait --no-build --pull never postgres
# A run of this script that was ended by force can have left a container that still
# unpacks or replaces files. The containers of this script have a label; one that still
# runs is ended before the staging starts.
project=$(docker inspect --format '{{index .Config.Labels "com.docker.compose.project"}}' "$(compose ps -q postgres)")
earlier=$(docker ps -q --filter "label=com.docker.compose.project=$project" --filter "label=junto.script=restore")
if [ -n "$earlier" ]; then
    echo "restore: a container of an earlier run of this script still runs. It is ended." >&2
    # shellcheck disable=SC2086
    docker rm -f $earlier > /dev/null
fi
once() {
    compose run --rm --no-deps --pull never -T --label junto.script=restore "$@"
}

sql() {
    compose exec -T postgres psql -U junto -d postgres -v ON_ERROR_STOP=1 -q -A -t
}
remove_staging() {
    once --entrypoint sh server -c \
        'rm -rf /var/lib/junto/attachments/.restore /var/lib/junto/recordings/.restore' < /dev/null || true
    compose exec -T postgres dropdb -U junto --maintenance-db=postgres --if-exists --force junto_restore < /dev/null || true
}
replacing=""
replaced=""
role=""
# The message of a failure follows the state of the database server at that moment, not
# the step that reported the failure: a command can fail after its change was made.
give_up() {
    code=$?
    if [ "$code" -eq 0 ]; then
        exit 0
    fi
    if [ -n "$replaced" ]; then
        echo "restore: the data was replaced, and the restore is complete. The list of the directories that no row names did not end. Start the deployment: docker compose -f $form up -d" >&2
        exit "$code"
    fi
    now=$(echo "select (select count(*) from pg_database where datname = 'junto') || ' ' || (select count(*) from pg_database where datname = 'junto_restore');" | sql 2>/dev/null) || now=unknown
    if [ "$now" = "1 0" ] && [ -n "$replacing" ]; then
        echo "restore: the data was replaced, and the last step reported a failure. Run the restore again." >&2
    elif [ "$now" = "1 0" ] || [ "$now" = "1 1" ]; then
        remove_staging
        echo "restore: failed before anything was replaced. The database and the files are as they were.${role:+ The role of the server has the password of $settings.}" >&2
    elif [ "$now" = "0 0" ] || [ "$now" = "0 1" ]; then
        echo "restore: failed, and the database \"junto\" does not exist: a restore has deleted it and has not ended. The server does not start. Run the restore again with a complete backup." >&2
    else
        echo "restore: failed, and the state of the database server could not be read. Run the restore again." >&2
    fi
    exit "$code"
}
trap give_up EXIT
trap 'exit 130' INT TERM

# Step 1: the dump is loaded into a new database, the server binary prepares that database
# for the start after a restore, and the archive is unpacked into a staging directory of
# each volume. Nothing of the deployment is replaced yet.
remove_staging
compose exec -T postgres createdb -U junto --maintenance-db=postgres junto_restore < /dev/null
# The role of the server gets the password of the settings file, whatever password the
# database server of this host had, and the staged database (ADR 0022). The role then loads
# the dump, so every object of the database is its own.
compose exec -T postgres psql -U junto -d junto_restore -v ON_ERROR_STOP=1 -q --single-transaction -f - \
    < images/postgres/server-role.sql
role=1
compose exec -T postgres pg_restore -U junto_server -d junto_restore --no-owner --exit-on-error < "$backup/database.dump"
# New epoch of the event stream, no backup that holds deletions back, no export
# (ADR 0018). The command gets the address of the staged database from the address in the
# settings of the server.
if ! once --entrypoint bash server -c '
    address=${JUNTO_DATABASE_URL%%\?*}
    options=${JUNTO_DATABASE_URL#"$address"}
    JUNTO_DATABASE_URL="${address%/*}/junto_restore$options" exec junto-server restored
' < /dev/null > /dev/null; then
    echo "restore: the server of this checkout did not accept the database of the backup; its message is above. A backup is restored with the checkout of the release that wrote it; upgrade afterwards." >&2
    exit 1
fi
once --entrypoint sh server -c '
    set -eu
    cd /var/lib/junto
    mkdir attachments/.restore recordings/.restore
    # The names of the files and of hard links get the staging directory, not the targets
    # of symbolic links.
    tar --numeric-owner -xpf - \
        --transform "s,^attachments\(/\|\$\),attachments/.restore\1,rSh" \
        --transform "s,^recordings\(/\|\$\),recordings/.restore\1,rSh"
' < "$backup/files.tar"
# The replacement empties the temporary directory of the recorder in a container of the
# recorder. That container must be able to start before anything is replaced.
if ! once --entrypoint sh recorder -c 'test -w /home/egress/tmp' < /dev/null; then
    echo "restore: a container of the recorder cannot be started, or cannot write its temporary directory" >&2
    exit 1
fi

# Step 2: the staged copies take the places of the data. The database "junto" is deleted
# first and the staged database gets its name last, so between the two no server starts.
# The files are replaced by renames and deletions, which need no further disk space. What
# belongs to the replaced data and to no row of the restored database is deleted: the
# exports, the transcription jobs of the spool, the partial files in the temporary directory
# of the recorder, and the copy of the replaced database from before a migration.
#
# A start of the server that was stopped while it made its copy leaves a database without a
# comment, and only the comment of the replaced database states its name (ADR 0018). That
# database is deleted first, while the comment exists: a run of this script that is ended
# after the next step has no other record of it.
left=$(printf '%s\n' "select datname from pg_database d where shobj_description(d.oid, 'pg_database') is null and datname = (select substring(shobj_description(oid, 'pg_database') from '^junto: the server makes the copy (junto_making_[0-9]+)') from pg_database where datname = 'junto');" | sql)
for database in $left; do
    compose exec -T postgres dropdb -U junto --maintenance-db=postgres --if-exists --force "$database" < /dev/null
done
compose exec -T postgres dropdb -U junto --maintenance-db=postgres --if-exists --force junto < /dev/null
replacing=1
once --entrypoint sh server -c '
    set -eu
    for volume in /var/lib/junto/attachments /var/lib/junto/recordings; do
        find "$volume" -mindepth 1 -maxdepth 1 ! -name .restore -exec rm -rf {} +
        find "$volume/.restore" -mindepth 1 -maxdepth 1 -exec mv -t "$volume" {} +
        rmdir "$volume/.restore"
    done
    find /var/lib/junto/exports /var/lib/junto/spool -mindepth 1 -maxdepth 1 -exec rm -rf {} +
' < /dev/null
once --entrypoint sh recorder -c 'find /home/egress/tmp -mindepth 1 -delete' < /dev/null
copies=$(printf "select datname from pg_database d where datname ~ '^junto_(before|making)_[0-9]+\$' and shobj_description(d.oid, 'pg_database') = '%s';\n" "$copy_comment" | sql)
for copy in $copies; do
    compose exec -T postgres dropdb -U junto --maintenance-db=postgres --if-exists --force "$copy" < /dev/null
done
echo 'alter database junto_restore rename to junto;' | sql
replacing=""
replaced=1

if [ -f "$backup/left-out" ] && [ -s "$backup/left-out" ]; then
    echo "restore: the backup does not hold $(wc -l < "$backup/left-out" | tr -d ' ') directories that its database names. The file left-out of the backup lists them."
fi
# The archive can hold a directory that its database does not name: an upload or a
# recording that ended between the copy of the database and the list of the files. The
# restore is complete without this step, so its failure is a message only.
if unnamed=$(once server unnamed-files < /dev/null); then
    if [ -n "$unnamed" ]; then
        echo "restore: the restored files hold $(printf '%s\n' "$unnamed" | wc -l | tr -d ' ') directories that no row of the restored database names:"
        printf '%s\n' "$unnamed"
        echo "restore: no route returns them. After the start, the server removes a directory of a line \"unnamed\" with: docker compose -f $form exec server junto-server unnamed-files --remove"
        echo "restore: a line \"skip ... changed_recently\" names a directory that was changed in the last 24 hours. The server cannot tell it from a directory that it writes, and removes it only after that time. deploy/backup.sh runs the removal at the start of each backup."
    fi
else
    echo "restore: the server could not list the directories that no row of the restored database names (junto-server unnamed-files). The restore is complete." >&2
fi
echo "restore: done. Start the deployment: docker compose -f $form up -d"
