#!/bin/sh
# Tests an upgrade of the local form from an earlier release, and the way back (ADR 0018,
# ADR 0021, ADR 0911). The earlier release is the commit of the deployment before the
# server copied its database before a migration; its files come from "git archive", so
# the checkout needs that commit.
# - the earlier release runs and gets messages with attachments;
# - the new server with a database user that may not create a database does not start,
#   names the cause, and migrates nothing;
# - a migration that fails: a copy exists, the way back with deploy/upgrade-undo.sh, and
#   the earlier release runs on the data again;
# - the upgrade: the server copies the database, migrates, and serves the same content;
# - the way back after a successful upgrade, and the upgrade once more;
# - a backup that the earlier release wrote is refused by deploy/restore.sh before anything
#   is replaced.
# Only the database and the server are started. Everything is removed at the end.
#
#   deploy/tests/upgrade.sh [<project name>]
#
# JUNTO_UPGRADE_FROM names another commit of an earlier release.
set -eu
project=${1:-junto-upgrade-test}
earlier=${JUNTO_UPGRADE_FROM:-2cadb0cf4236490056f650c7600514610e22b84f}
deploy=$(cd "$(dirname "$0")/.." && pwd)
work=$(mktemp -d "${TMPDIR:-/tmp}/$project.XXXXXX")
port=${JUNTO_LOCAL_PORT:-8080}
cd "$deploy"

mkdir "$work/models" "$work/earlier"
(
    umask 077
    {
        ./generate-secrets.sh
        echo "JUNTO_MODELS_DIR=$work/models"
        echo "JUNTO_LOCAL_PORT=$port"
        echo "JUNTO_POSTGRES_MEMORY=512m"
        echo "JUNTO_SERVER_MEMORY=512m"
    } > "$work/env"
)
JUNTO_TEST_LIMITED_PASSWORD=$(openssl rand -hex 16)
export JUNTO_ENV_FILE="$work/env" COMPOSE_PROJECT_NAME="$project" JUNTO_TEST_LIMITED_PASSWORD
new() {
    docker compose --env-file "$work/env" -f local.yaml "$@"
}
old() {
    docker compose --env-file "$work/env" -f "$work/earlier/deploy/local.yaml" "$@"
}
driver() {
    step=$1
    shift
    python3 -B "$deploy/tests/content_driver.py" "$step" --project "$project" --env-file "$work/env" \
        --compose-file local.yaml --origin "http://127.0.0.1:$port" --state "$work/state.json" "$@"
}
remove() {
    code=$?
    if [ "$code" -ne 0 ]; then
        new ps -a || true
        new logs --tail 30 server postgres || true
    fi
    new down -v --timeout 20 > /dev/null 2>&1 || true
    rm -rf "$work"
    exit "$code"
}
trap remove EXIT
trap 'exit 130' INT TERM

failures=0
check() {
    if [ "$2" = "$3" ]; then
        echo "ok    $1"
    else
        echo "FAIL  $1: got \"$2\", expected \"$3\""
        failures=$((failures + 1))
    fi
}
sql() {
    new exec -T postgres psql -U junto -d "${2:-junto}" -At -v ON_ERROR_STOP=1 -c "$1" < /dev/null
}
schema() {
    sql 'select max(version) from _sqlx_migrations where success'
}
epoch() {
    sql 'select epoch from event_stream'
}
copies() {
    sql "select string_agg(datname, ' ' order by datname) from pg_database where datname like 'junto\\_before\\_%'" postgres
}
# Waits until the log of the server holds a text, for 3 minutes at most. The server does
# not start in these cases, so its container starts again and again.
log_holds() {
    tries=0
    until new logs --no-log-prefix --since "$since" server 2>/dev/null | grep -q -F "$1"; do
        tries=$((tries + 1))
        if [ "$tries" -ge 90 ]; then
            echo no
            return
        fi
        sleep 2
    done
    echo yes
}
healthy() {
    docker inspect --format '{{.State.Health.Status}}' "$(new ps -a -q server)"
}

# Both releases build an image with the same name, so every start names its build. A build
# that was made before comes from the cache.
git -C "$deploy/.." archive "$earlier" | tar -x -C "$work/earlier"
# The way back runs in the checkout of the earlier release. That commit is older than the
# script, so the test puts the script there.
cp upgrade-undo.sh "$work/earlier/deploy/upgrade-undo.sh"

# The earlier release, with content.
old up -d --build --wait --wait-timeout 600 postgres server
driver seed --count 3
before=$(schema)
old stop server

# A backup as the earlier release wrote it: the database, the two directories, the settings.
mkdir "$work/earlier-backup"
old exec -T postgres pg_dump -U junto -d junto --format=custom < /dev/null > "$work/earlier-backup/database.dump"
old run --rm --no-deps -T --entrypoint tar server --numeric-owner -C /var/lib/junto -cf - attachments recordings \
    < /dev/null > "$work/earlier-backup/files.tar"
cp "$work/env" "$work/earlier-backup/env"
(cd "$work/earlier-backup" && { sha256sum database.dump files.tar env 2>/dev/null || shasum -a 256 database.dump files.tar env; } > SHA256SUMS)

# A database user that may not create a database. It is a member of the role that owns the
# tables, so it could migrate.
printf "create role junto_limited login nocreatedb in role junto password '%s';\n" "$JUNTO_TEST_LIMITED_PASSWORD" \
    | new exec -T postgres psql -U junto -d postgres -q -v ON_ERROR_STOP=1
since=$(date -u +%Y-%m-%dT%H:%M:%SZ)
new -f tests/upgrade-limited-user.yaml up -d --build --no-deps server
check "without the right to create a database the server names the cause" "$(log_holds 'the database user may create a database')" "yes"
check "its log holds the message of the database" "$(log_holds 'permission denied to create database')" "yes"
check "that server is not healthy" "$(healthy | grep -c '^healthy$' || true)" "0"
new stop server
check "nothing was migrated" "$(schema)" "$before"
check "no copy was made" "$(copies)" ""

# A migration that fails: a table of the new schema exists already. The container is
# started once and not again, so that the stop of the test does not end a start that is
# making the copy.
sql 'create table purges (id integer)' > /dev/null
since=$(date -u +%Y-%m-%dT%H:%M:%SZ)
new build server > /dev/null 2>&1
new up --no-start --no-deps server
docker update --restart no "$(new ps -a -q server)" > /dev/null
new start server
check "a migration that fails is named in the log" "$(log_holds 'database migration failed')" "yes"
check "that server is not healthy" "$(healthy | grep -c '^healthy$' || true)" "0"
new stop server
check "the failed migration left the schema as it was" "$(schema)" "$before"
check "the server made a copy before the migration" "$(copies)" "junto_before_$before"

# The way back: the copy becomes the database, and the earlier release runs on it.
epoch_before=$(epoch)
"$work/earlier/deploy/upgrade-undo.sh" local.yaml > "$work/undo.log" 2>&1 || { cat "$work/undo.log"; exit 1; }
check "after the way back no copy is left" "$(copies)" ""
check "the way back gave the event stream a new epoch" "$([ "$(epoch)" != "$epoch_before" ] && echo new)" "new"
check "the database that the newer version changed is kept" "$(sql "select count(*) from pg_database where datname = 'junto_undone'" postgres)" "1"
old up -d --build --wait --wait-timeout 600 postgres server
driver check --count 3 || failures=$((failures + 1))
old stop server

# The upgrade. The table that stopped the migration is in the copy too.
sql 'drop table purges' > /dev/null
since=$(date -u +%Y-%m-%dT%H:%M:%SZ)
new up -d --build --wait --wait-timeout 600 postgres server
after=$(schema)
check "the upgrade applied a migration" "$([ "$after" -gt "$before" ] && echo newer)" "newer"
check "the server made a copy before the migration" "$(copies)" "junto_before_$before"
check "the log names the copy" "$(log_holds "junto_before_$before")" "yes"
check "the health check of the image is the command of the server" \
    "$(docker inspect --format '{{json .Config.Healthcheck.Test}}' "$(new ps -q server)")" '["CMD","junto-server","health"]'
check "the upgraded server is healthy" "$(healthy)" "healthy"
check "the server can write into the directory of the exports" "$(new exec -T server sh -c 'test -w "$JUNTO_EXPORTS_DIR" && echo yes' < /dev/null)" "yes"
driver check --count 3 || failures=$((failures + 1))
driver send || failures=$((failures + 1))
driver check --count 4 || failures=$((failures + 1))

# The way back after an upgrade that worked: the content from after the upgrade is not in
# the copy.
status=0
"$work/earlier/deploy/upgrade-undo.sh" local.yaml "junto_before_$before" > "$work/undo.log" 2>&1 || status=$?
check "a second way back is refused while the database of the first is kept" \
    "$([ "$status" -ne 0 ] && grep -c 'junto_undone exists' "$work/undo.log")" "1"
check "that refusal changed no database" "$(copies) $(sql "select count(*) from pg_database where datname in ('junto', 'junto_undone')" postgres)" "junto_before_$before 2"
new exec -T postgres dropdb -U junto junto_undone < /dev/null
epoch_before=$(epoch)
"$work/earlier/deploy/upgrade-undo.sh" local.yaml "junto_before_$before" > "$work/undo.log" 2>&1 || { cat "$work/undo.log"; exit 1; }
check "the way back gave the event stream a new epoch" "$([ "$(epoch)" != "$epoch_before" ] && echo new)" "new"
old up -d --build --wait --wait-timeout 600 postgres server
check "the earlier release runs on the schema of the copy" "$(schema)" "$before"
driver check --count 3 || failures=$((failures + 1))
old stop server

# The upgrade once more.
new up -d --build --wait --wait-timeout 600 postgres server
check "the second upgrade applied the migration" "$(schema)" "$after"
check "one copy exists" "$(copies)" "junto_before_$before"
driver check --count 3 || failures=$((failures + 1))

# A backup of the earlier release: the server of this release cannot prepare its database,
# and the restore ends before anything is replaced.
identifier=$(curl -fsS --max-time 10 "http://127.0.0.1:$port/api/server" | sed -n 's/.*"server_id":"\([^"]*\)".*/\1/p')
status=0
./restore.sh local.yaml "$work/earlier-backup" > "$work/restore.log" 2>&1 || status=$?
check "a backup of the earlier release is refused with the reason" \
    "$([ "$status" -ne 0 ] && grep -c 'A backup of an earlier release is restored with the checkout of that release' "$work/restore.log")" "1"
check "nothing was replaced" "$(grep -c 'failed before anything was replaced. The database and the files are as they were' "$work/restore.log")" "1"
new up -d --build --wait --wait-timeout 600 postgres server
check "the server is the same after the refused restore" \
    "$(curl -fsS --max-time 10 "http://127.0.0.1:$port/api/server" | sed -n 's/.*"server_id":"\([^"]*\)".*/\1/p')" "$identifier"
driver check --count 3 || failures=$((failures + 1))

# A backup with the server stopped, as the upgrade procedure makes it, and its restore. The
# restore deletes the copy from before the migration: it is a copy of the replaced database.
new stop server
./backup.sh local.yaml "$work/backup" > "$work/backup.log" 2>&1 || { cat "$work/backup.log"; exit 1; }
check "a backup with the server stopped has its five files" "$(cd "$work/backup" && LC_ALL=C ls -A | tr '\n' ' ')" "SHA256SUMS database.dump env files.tar left-out "
check "the server is still stopped" "$(new ps --status running --format '{{.Service}}' server)" ""
check "no backup holds deletions back after it" "$(sql 'select count(*) from backups')" "0"
new up -d --build --wait --wait-timeout 600 postgres server
driver send || failures=$((failures + 1))
driver check --count 4 || failures=$((failures + 1))
./restore.sh local.yaml "$work/backup" > "$work/restore.log" 2>&1 || { cat "$work/restore.log"; exit 1; }
check "the restore deleted the copy from before the migration" "$(copies)" ""
new up -d --build --wait --wait-timeout 600 postgres server
driver check --count 3 || failures=$((failures + 1))

if [ "$failures" -eq 0 ]; then
    echo "all checks passed"
else
    echo "$failures check(s) failed"
    exit 1
fi
