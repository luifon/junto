#!/bin/sh
# Goes back to the copy of the database that the server made before a migration
# (docs/install.md, "The way back after an upgrade"; ADR 0018, ADR 0021). Run it in the
# checkout of the earlier release:
#
#   git checkout <earlier release>
#   deploy/upgrade-undo.sh <form file> [<name of the copy>]
#
# The server image of this checkout is built, and its command "new-event-epoch" runs on the
# copy: the clients hold positions of events that the copy does not have, and a new epoch
# makes every client load a new snapshot (ADR 0010). Then the copy takes the name of the
# database, and the database that the newer version changed is kept under the name
# "junto_undone"; both renames are one transaction. A failure before the renames changes
# no name, and the script can be run again.
#
# A checkout that is older than this script does not have it: copy the script of the newer
# release into deploy/ of that checkout.
#
# The server and the worker are stopped and stay stopped. Start the deployment afterwards
# with "docker compose -f <form file> up -d --build". Without a name, the copy is the one
# that the comment of the database names, which the server writes for an upgrade that has
# not finished, and otherwise the one database "junto_before_<number>" that has the comment
# of the server.
#
# The copy holds the database only. A file that the newer version deleted is not brought
# back; for that case, restore the backup that was made before the upgrade. A directory
# that was written after the upgrade stays on the disk without a row; the script lists
# these directories when the server of the checkout has the command "unnamed-files".
set -eu

if [ "$#" -lt 1 ] || [ "$#" -gt 2 ]; then
    echo "usage: $0 <form file> [<name of the copy>]" >&2
    exit 2
fi
deploy=$(cd "$(dirname "$0")" && pwd)
form=$(basename "$1")
settings=${JUNTO_ENV_FILE:-$deploy/.env}
cd "$deploy"

if [ ! -f "$form" ] || [ ! -f "$settings" ]; then
    echo "upgrade-undo: $deploy/$form or $settings does not exist" >&2
    exit 2
fi
compose() {
    docker compose --env-file "$settings" -f "$form" "$@"
}
sql() {
    compose exec -T postgres psql -U junto -d postgres -v ON_ERROR_STOP=1 -q -A -t
}
# The comment that the server gives a copy of this database before a migration (ADR 0018).
copy_comment='junto: copy of the database junto before a migration'

compose stop server worker
compose up -d --wait postgres

named=$(echo "select datname from pg_database where datname ~ '^junto_before_[0-9]+\$' order by datname;" | sql)
marked=$(printf "select datname from pg_database d where datname ~ '^junto_before_[0-9]+\$' and shobj_description(d.oid, 'pg_database') = '%s' order by datname;\n" "$copy_comment" | sql)
# While an upgrade of the database has not applied every migration, the first line of its
# comment names the copy from before the upgrade (ADR 0018).
stated=$(printf '%s\n' "select substring(shobj_description(oid, 'pg_database') from '^junto: an upgrade of this database has not finished[.] (junto_before_[0-9]+) is the copy from before the upgrade') from pg_database where datname = 'junto';" | sql)
copy=${2:-${stated:-$marked}}
# What a start of the server left that was stopped while it made its copy: a database with
# the comment of a copy under the name "junto_making_<number>", and a database of such a
# name without a comment that the comment of the database states as the copy that the
# server makes. The next start of the server deletes both by that comment, which is on
# junto_undone after the renames.
left=$(printf "select datname from pg_database d where datname ~ '^junto_making_[0-9]+\$' and (shobj_description(d.oid, 'pg_database') = '%s' or (shobj_description(d.oid, 'pg_database') is null and datname = (select substring(shobj_description(oid, 'pg_database') from '^junto: the server makes the copy (junto_making_[0-9]+)') from pg_database where datname = 'junto'))) order by datname;\n" "$copy_comment" | sql)
case "$copy" in
    "")
        echo "upgrade-undo: the database server has no copy that the server made before a migration. Restore the backup that was made before the upgrade." >&2
        if [ -n "$named" ]; then
            echo "upgrade-undo: these databases have the name of a copy and not its comment. An earlier version of the server left such a copy when it was stopped while it made the copy. To use one, name it:" >&2
            printf '%s\n' "$named" >&2
        fi
        exit 1
        ;;
    junto_before_*[!0-9]* | junto_before_)
        echo "upgrade-undo: more than one copy exists, or the name is not junto_before_<number>. Name the copy that the log of the server stated:" >&2
        printf '%s\n' "$marked" >&2
        exit 1
        ;;
    junto_before_*) ;;
    *)
        echo "upgrade-undo: \"$copy\" is not a name of the form junto_before_<number>" >&2
        exit 1
        ;;
esac
if ! printf '%s\n' "$named" | grep -q -x -F "$copy"; then
    echo "upgrade-undo: the database server has no database $copy" >&2
    if [ "$copy" = "$stated" ]; then
        echo "upgrade-undo: the comment of the database junto names it as the copy from before an upgrade that has not finished. It was deleted. Restore the backup that was made before the upgrade." >&2
    fi
    exit 1
fi
if [ "$(echo "select count(*) from pg_database where datname = 'junto_undone';" | sql)" != 0 ]; then
    echo "upgrade-undo: the database junto_undone exists: an earlier way back kept it. Nothing was changed. Delete it first: docker compose -f $form exec postgres dropdb -U junto junto_undone" >&2
    exit 1
fi

compose build server
# The role as which the server of this checkout connects gets the password of the settings
# file, as a restore gives it. A release before the role "junto_server" connects as the
# first user, and a change of the secret in a later release did not change the password of
# that user (ADR 0022). The password comes from the variable of the PostgreSQL container.
user=$(compose run --rm --no-deps -T --entrypoint bash server -c '
    address=${JUNTO_DATABASE_URL#*://}
    printf "%s\n" "${address%%[:@]*}"
' < /dev/null)
case "$user" in
    "" | *[!a-z0-9_]*)
        echo "upgrade-undo: the settings of the server of this checkout do not name a database role that this script reads. Nothing was changed." >&2
        exit 1
        ;;
esac
if [ "$(echo "select count(*) from pg_roles where rolname = '$user';" | sql)" != 1 ]; then
    echo "upgrade-undo: the database server has no role $user, as which the server of this checkout connects. Nothing was changed." >&2
    exit 1
fi
# The statement holds the password; the settings keep it out of the log of PostgreSQL and
# out of the output of psql when the statement fails.
printf '%s\n' '\getenv password POSTGRES_PASSWORD' '\set VERBOSITY terse' \
    "set log_min_error_statement = 'panic'; set log_statement = 'none'; set log_min_duration_statement = -1;" \
    "alter role \"$user\" password :'password';" | sql
echo "upgrade-undo: the database role $user, as which the server of this checkout connects, has the password of $settings"
# The command gets the address of the copy from the address in the settings of the server.
compose run --rm --no-deps -T --entrypoint bash server -c '
    address=${JUNTO_DATABASE_URL%%\?*}
    options=${JUNTO_DATABASE_URL#"$address"}
    JUNTO_DATABASE_URL="${address%/*}/$1$options" exec junto-server new-event-epoch
' bash "$copy" < /dev/null > /dev/null

sql <<SQL
begin;
alter database junto rename to junto_undone;
alter database $copy rename to junto;
comment on database junto is null;
commit;
SQL
echo "upgrade-undo: the database is the copy $copy again, with a new epoch of the event stream. The database that the newer version changed is kept as junto_undone; delete it when it is not needed: docker compose -f $form exec postgres dropdb -U junto junto_undone"

for database in $left; do
    echo "upgrade-undo: the database $database is what a start of the server left that was stopped while it made its copy. A later upgrade can refuse to start while it exists. Delete it: docker compose -f $form exec postgres dropdb -U junto $database"
done

# The directories that were written after the upgrade have no row in the copy. The command
# is run with the server of this checkout; the way back is complete without it, so its
# failure is a message only.
if unnamed=$(compose run --rm --no-deps -T server unnamed-files < /dev/null); then
    if [ -n "$unnamed" ]; then
        echo "upgrade-undo: the disk holds $(printf '%s\n' "$unnamed" | wc -l | tr -d ' ') directories that no row of the copy names:"
        printf '%s\n' "$unnamed"
        echo "upgrade-undo: no route returns them. After the start, the server removes a directory of a line \"unnamed\" with: docker compose -f $form exec server junto-server unnamed-files --remove"
        echo "upgrade-undo: a line \"skip ... changed_recently\" names a directory that was changed in the last 24 hours. The server cannot tell it from a directory that it writes, and removes it only after that time. deploy/backup.sh runs the removal at the start of each backup."
    fi
else
    echo "upgrade-undo: the server of this checkout did not list the directories that no row of the copy names. A release without the command \"unnamed-files\" cannot; the directories that were written after the upgrade then stay on the disk." >&2
fi
echo "upgrade-undo: start the deployment: docker compose -f $form up -d --build"
