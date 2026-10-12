#!/bin/sh
# Brings a deployment that an earlier release created over to the database role of the
# server (docs/install.md, "The database user of an earlier release"; ADR 0022). In such a
# deployment the server connected as the first user of the PostgreSQL container, which has
# every right, and that user owns the database and every table. This script creates the
# role "junto_server", which may log in and create a database and nothing else, gives it
# the password of the settings file, and makes it the owner of the database "junto", of
# each copy from before a migration, and of the tables, views, sequences, routines, types
# and schemas in them. Run it in the checkout of the new release, with the server stopped,
# before the first start of that release:
#
#   deploy/database-user.sh <form file>
#
# A second run changes nothing. Each database changes in one transaction, so a failure
# leaves it as it was. Start the deployment afterwards with
# "docker compose -f <form file> up -d --build".
#
# JUNTO_ENV_FILE names another settings file than deploy/.env. COMPOSE_PROJECT_NAME names
# another project than the one of the form file.
set -eu

if [ "$#" -ne 1 ]; then
    echo "usage: $0 <form file>" >&2
    exit 2
fi
deploy=$(cd "$(dirname "$0")" && pwd)
form=$(basename "$1")
settings=${JUNTO_ENV_FILE:-$deploy/.env}
cd "$deploy"

if [ ! -f "$form" ] || [ ! -f "$settings" ]; then
    echo "database-user: $deploy/$form or $settings does not exist" >&2
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

# A change of the owner of a table waits for every statement that uses the table. A server
# that does not start is started again and again by its restart rule.
if [ -n "$(compose ps -a --status running --status restarting --format '{{.Service}}' server 2>/dev/null)" ]; then
    echo "database-user: the server runs. Stop it first: docker compose -f $form stop server" >&2
    exit 1
fi
# The container gets the Compose file of this checkout: the password of the settings file,
# and the rules that let only the role of the server connect over the network.
compose up -d --wait postgres

databases=$(printf "select datname from pg_database d where datname = 'junto' or (datname ~ '^junto_before_[0-9]+\$' and shobj_description(d.oid, 'pg_database') = '%s') order by datname;\n" "$copy_comment" | sql)
if ! printf '%s\n' "$databases" | grep -q -x junto; then
    echo "database-user: the database server has no database junto. Nothing was changed. When a restore has not ended, run it again: it gives the restored database to the role." >&2
    exit 1
fi
for database in $databases; do
    compose exec -T postgres psql -U junto -d "$database" -v ON_ERROR_STOP=1 -q --single-transaction -f - \
        < images/postgres/server-role.sql
    echo "database-user: the role junto_server owns the database $database and what the first user owned in it"
done
echo "database-user: done. The server connects as junto_server with the password of $settings. Start the deployment: docker compose -f $form up -d --build"
