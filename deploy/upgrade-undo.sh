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
# database "junto_before_<number>" that has the comment of the server.
#
# The copy holds the database only. A file that the newer version deleted is not brought
# back; for that case, restore the backup that was made before the upgrade.
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
copy=${2:-$marked}
case "$copy" in
    "")
        echo "upgrade-undo: the database server has no copy that the server made before a migration. Restore the backup that was made before the upgrade." >&2
        if [ -n "$named" ]; then
            echo "upgrade-undo: these databases have the name of a copy and not its comment. A copy has no comment when the server was stopped while it made the copy. To use one, name it:" >&2
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
    exit 1
fi
if [ "$(echo "select count(*) from pg_database where datname = 'junto_undone';" | sql)" != 0 ]; then
    echo "upgrade-undo: the database junto_undone exists: an earlier way back kept it. Nothing was changed. Delete it first: docker compose -f $form exec postgres dropdb -U junto junto_undone" >&2
    exit 1
fi

compose build server
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
echo "upgrade-undo: start the deployment: docker compose -f $form up -d --build"
