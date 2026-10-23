#!/bin/sh
# Tests an upgrade of the local form from an earlier release, and the way back (ADR 0018,
# ADR 0021, ADR 0022, ADR 0911). The earlier release is the commit of the deployment before
# the server copied its database before a migration; its server connects as the first user
# of the database server, and its files come from "git archive", so the checkout needs that
# commit.
# - the earlier release runs and gets messages with attachments;
# - the new server does not start before deploy/database-user.sh ran, and names the cause;
# - that script gives the database to the role of the server; a second run changes nothing;
#   from here on PostgreSQL writes every statement to its log, and the log holds no password,
#   also when the statement that sets the password fails, in that script and in the way back;
# - a backup with the server image of this release on the database of the earlier one is
#   refused;
# - the new server with a role that may not create a database does not start, names the
#   cause, and migrates nothing;
# - an upgrade that fails in its second migration: the first one stays applied, the copy
#   from before the upgrade stays over a second start, the comment of the database names
#   it, the way back with deploy/upgrade-undo.sh takes it, and the earlier release runs on
#   the data again;
# - a start that is stopped while PostgreSQL creates the copy, and the upgrade after it: the
#   server copies the database, migrates, and serves the same content;
# - the way back after a successful upgrade, which gets the stop signal while it lists the
#   directories without a row, and the upgrade once more;
# - a backup that the earlier release wrote is refused by deploy/restore.sh before anything
#   is replaced;
# - a restore after a start that was stopped while PostgreSQL created the copy, which is ended
#   between its two deletions, and its second run;
# - a way back inside this release, to a copy that the test makes: the script lists the
#   directory that was written after the copy, and the server removes it when it is old.
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
        echo "JUNTO_SERVER_IMAGE=$project-server"
        echo "JUNTO_INIT_IMAGE=$project-init"
    } > "$work/env"
)
export JUNTO_ENV_FILE="$work/env" COMPOSE_PROJECT_NAME="$project"
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
comment() {
    sql "select coalesce(shobj_description(oid, 'pg_database'), '') from pg_database where datname = '$1'" postgres
}
# The owner of the database $1, and the number of its tables, sequences and routines that
# another role than the role of the server owns.
owners() {
    sql "select (select datdba::regrole from pg_database where datname = current_database()), (select count(*) from pg_class where relnamespace = 'public'::regnamespace and relowner <> r) + (select count(*) from pg_proc where pronamespace = 'public'::regnamespace and proowner <> r) from (select oid as r from pg_roles where rolname = 'junto_server') role" "$1"
}
role() {
    sql "select rolcanlogin, rolcreatedb, rolsuper, rolcreaterole, rolreplication, rolbypassrls from pg_roles where rolname = 'junto_server'" postgres
}
# The values of the secrets of the settings file that a log holds.
secrets_in() {
    found=0
    for value in $(sed -n -E 's/^JUNTO_[A-Z_]*(PASSWORD|SECRET|API_KEY)=//p' "$work/env"); do
        if grep -q -F "$value" "$1"; then
            found=$((found + 1))
        fi
    done
    echo "$found"
}
# The log of PostgreSQL since a time.
postgres_log() {
    new logs --no-log-prefix --since "$1" postgres > "$work/postgres.log" 2>&1
}
# The databases that a stopped start of the server can leave.
making() {
    sql "select string_agg(datname, ' ' order by datname) from pg_database where datname like 'junto\\_making\\_%'" postgres
}
# Waits until a command prints a value, for 60 seconds at most.
wait_until() {
    tries=0
    until [ "$(eval "$2")" = "$3" ]; do
        tries=$((tries + 1))
        if [ "$tries" -ge 300 ]; then
            echo "FAIL  after 60 s this did not hold: $1"
            exit 1
        fi
        sleep 0.2
    done
}
sessions() {
    sql "select count(*) from pg_stat_activity where $1" postgres
}
# A program named "docker" in front of the real one, for the script under test. It runs a
# step of the test before the one call whose arguments match a pattern (ADR 0911). A new
# file takes the place of the program, because a shell reads its script while it runs.
real=$(command -v docker)
mkdir "$work/bin"
in_front() {
    cat > "$work/bin/docker.next" <<EOF
#!/bin/sh
case "\$*" in
    $1)
$2
        ;;
esac
exec "$real" "\$@"
EOF
    chmod +x "$work/bin/docker.next"
    mv "$work/bin/docker.next" "$work/bin/docker"
}
# Runs a script with the program in front of docker, and stores its process identifier, so
# that a step of the program can end the script. Prints the exit code.
in_background() {
    log=$1
    shift
    PATH="$work/bin:$PATH" "$@" > "$log" 2>&1 &
    echo "$!" > "$work/script"
    code=0
    wait "$!" || code=$?
    echo "$code"
}
# Stops a start of the server of this release while PostgreSQL creates the copy before a
# migration: after the server wrote the name of the copy into the comment of the database,
# and before the copy has its own comment (ADR 0018). PostgreSQL copies a database only
# while no other session is connected to it, so a session in the database holds the server
# in its retries. A second session then takes a lock on the database that the creation of
# the copy waits for, and the first session ends: the creation has begun, and waits, when
# the server container is killed. After the kill, the second session ends and PostgreSQL
# finishes the creation.
stop_during_copy() {
    new exec -T -e PGAPPNAME=junto-test-connected postgres psql -U junto -d junto -q -c 'select pg_sleep(600)' \
        < /dev/null > /dev/null 2>&1 &
    connected=$!
    wait_until "a session is connected to the database" "sessions \"application_name = 'junto-test-connected' and datname = 'junto'\"" 1
    new up --no-start --no-deps server
    docker update --restart no "$(new ps -a -q server)" > /dev/null
    new start server
    wait_until "the server wrote the name of its copy into the comment of the database" "comment junto" \
        "junto: the server makes the copy $1"
    new exec -T -e PGAPPNAME=junto-test-lock postgres psql -U junto -d postgres -q -c begin \
        -c "comment on database junto is 'junto: the server makes the copy $1'" -c 'select pg_sleep(600)' \
        < /dev/null > /dev/null 2>&1 &
    held=$!
    wait_until "the second session holds a lock on the database" \
        "sql \"select count(*) from pg_locks l join pg_stat_activity a using (pid) where a.application_name = 'junto-test-lock' and l.locktype = 'object' and l.classid = 'pg_database'::regclass and l.objid = (select oid from pg_database where datname = 'junto') and l.granted\" postgres" 1
    sql "select pg_terminate_backend(pid) from pg_stat_activity where application_name = 'junto-test-connected'" postgres > /dev/null
    wait_until "the creation of the copy waits for the lock" \
        "sessions \"usename = 'junto_server' and wait_event_type = 'Lock' and query like 'create database %'\"" 1
    docker kill "$(new ps -q server)" > /dev/null
    wait_until "the server container has ended" "new ps --status running --format '{{.Service}}' server" ""
    sql "select pg_terminate_backend(pid) from pg_stat_activity where application_name = 'junto-test-lock'" postgres > /dev/null
    wait_until "the creation of the copy has ended" "sessions \"query like 'create database %' and state <> 'idle'\"" 0
    wait "$connected" || true
    wait "$held" || true
}

# Both releases build an image with the same name, so every start names its build. A build
# that was made before comes from the cache.
git -C "$deploy/.." archive "$earlier" | tar -x -C "$work/earlier"
# The way back runs in the checkout of the earlier release. That commit is older than the
# script, so the test puts the script there.
cp upgrade-undo.sh "$work/earlier/deploy/upgrade-undo.sh"

# The earlier release, with content.
old up -d --build --wait --wait-timeout 600 postgres server
new pull recorder
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

# A server of this release connects as the role "junto_server", which the deployment of
# the earlier release does not have.
since=$(date -u +%Y-%m-%dT%H:%M:%SZ)
new up -d --build postgres server
check "before the step for the database user the server names the role that cannot log in" \
    "$(log_holds 'password authentication failed for user "junto_server"')" "yes"
check "that server is not healthy" "$(healthy | grep -c '^healthy$' || true)" "0"
new stop server
check "nothing was migrated" "$(schema)" "$before"
check "no role of the server exists" "$(role)" ""

# The step for a deployment of an earlier release.
status=0
new start server
./database-user.sh local.yaml > "$work/user.log" 2>&1 || status=$?
check "the step is refused while the server runs" "$([ "$status" -ne 0 ] && grep -c 'the server runs. Stop it first' "$work/user.log")" "1"
new stop server
check "that refusal created no role" "$(role)" ""
# From here on PostgreSQL writes every statement to its log. It writes the statement of a
# failure also without this setting. The statement that gives the role of the server its
# password must not be in the log (ADR 0022).
sql "alter system set log_statement = 'all'" postgres > /dev/null
sql 'select pg_reload_conf()' postgres > /dev/null
logged=$(date -u +%Y-%m-%dT%H:%M:%SZ)
./database-user.sh local.yaml > "$work/user.log" 2>&1 || { cat "$work/user.log"; exit 1; }
check "the role of the server may log in and create a database, and has no other attribute" "$(role)" "t|t|f|f|f|f"
check "the role owns the database, and every table, sequence and routine in it" "$(owners junto)" "junto_server|0"
check "the output of the step holds no secret" "$(secrets_in "$work/user.log")" "0"
postgres_log "$logged"
check "the log of PostgreSQL holds the statements of the step" \
    "$(grep -c 'statement: alter database junto owner to junto_server' "$work/postgres.log")" "1"
check "the log of PostgreSQL holds no secret" "$(secrets_in "$work/postgres.log")" "0"
./database-user.sh local.yaml > "$work/user.log" 2>&1 || { cat "$work/user.log"; exit 1; }
check "a second run of the step ends without a failure" "$(grep -c 'database-user: done' "$work/user.log")" "1"
# In a database that takes no change, the statement that sets the password fails.
sql 'alter database junto set default_transaction_read_only = on' postgres > /dev/null
logged=$(date -u +%Y-%m-%dT%H:%M:%SZ)
status=0
./database-user.sh local.yaml > "$work/user.log" 2>&1 || status=$?
sql 'alter database junto reset default_transaction_read_only' postgres > /dev/null
postgres_log "$logged"
check "the step fails at the statement that sets the password" \
    "$([ "$status" -ne 0 ] && grep -c 'ERROR:  cannot execute ALTER ROLE in a read-only transaction' "$work/postgres.log")" "1"
check "that failure changed nothing" "$(role) $(owners junto)" "t|t|f|f|f|f junto_server|0"
check "the output of the failed step holds no secret" "$(secrets_in "$work/user.log")" "0"
check "the log of PostgreSQL holds no secret after that failure" "$(secrets_in "$work/postgres.log")" "0"

# The owner of a database can change what a name without a schema means in it. The step
# runs as the first user in that database, so it must not read a name of the role: here a
# view with the name of a system view, whose function records the user that runs it.
as_role() {
    new exec -T postgres psql -U junto_server -d junto -q -v ON_ERROR_STOP=1 "$@" < /dev/null
}
as_role -c 'create table trapped (by_user text)' \
    -c "create function trap() returns boolean language sql as 'insert into public.trapped values (current_user) returning true'" \
    -c 'create view pg_roles as select * from pg_catalog.pg_roles where public.trap()' \
    -c 'alter database junto set search_path = public, pg_catalog'
./database-user.sh local.yaml > "$work/user.log" 2>&1 || { cat "$work/user.log"; exit 1; }
check "the step runs nothing that the role put in the place of a system name" "$(sql 'select count(*) from public.trapped')" "0"
as_role -c 'alter database junto reset search_path' -c 'drop view public.pg_roles' -c 'drop function public.trap()' -c 'drop table public.trapped'

# The image of the server is the one of this release, and the database has the schema of
# the earlier one: a backup would state the wrong schema, so none is made.
status=0
./backup.sh local.yaml "$work/other-schema" > "$work/backup.log" 2>&1 || status=$?
check "a backup with the server image of another release than the database is refused" \
    "$([ "$status" -ne 0 ] && grep -c "and the database has the schema $before. A backup needs the server of the release that the database has" "$work/backup.log")" "1"
check "it leaves no directory" "$([ -e "$work/other-schema" ] && echo exists)" ""

# A role that may not create a database: the copy before a migration cannot be made.
sql 'alter role junto_server nocreatedb' postgres > /dev/null
since=$(date -u +%Y-%m-%dT%H:%M:%SZ)
new up -d --build --no-deps server
check "without the right to create a database the server names the cause" "$(log_holds 'the database user may create a database')" "yes"
check "its log holds the message of the database" "$(log_holds 'permission denied to create database')" "yes"
check "that server is not healthy" "$(healthy | grep -c '^healthy$' || true)" "0"
new stop server
check "nothing was migrated" "$(schema)" "$before"
check "no copy was made" "$(copies)" ""
./database-user.sh local.yaml > "$work/user.log" 2>&1 || { cat "$work/user.log"; exit 1; }
check "the step gives the role the right to create a database again" "$(role)" "t|t|f|f|f|f"

# An upgrade that fails in its second migration: a function of that migration exists
# already, so the first waiting migration is applied and the second is not. The container
# is started once and not again, so that the stop of the test does not end a start that is
# making the copy.
newest=$(new run --rm --no-deps -T server schema-version < /dev/null | sed -n 's/^schema //p')
as_role -c "create function is_inside_call(subject uuid, reader uuid) returns boolean language sql as 'select true'"
since=$(date -u +%Y-%m-%dT%H:%M:%SZ)
new build server > /dev/null 2>&1
new up --no-start --no-deps server
docker update --restart no "$(new ps -a -q server)" > /dev/null
new start server
check "a migration that fails is named in the log" "$(log_holds 'database migration failed')" "yes"
check "that server is not healthy" "$(healthy | grep -c '^healthy$' || true)" "0"
new stop server
partly=$(schema)
check "the failed upgrade applied a first part of the migrations" "$([ "$partly" -gt "$before" ] && [ "$partly" -lt "$newest" ] && echo part)" "part"
check "the server made a copy before the first migration" "$(copies)" "junto_before_$before"
check "the comment of the database names the copy of the upgrade that has not finished" "$(comment junto)" \
    "junto: an upgrade of this database has not finished. junto_before_$before is the copy from before the upgrade"
# A second start: the database is migrated in part, so the copy from before the upgrade
# stays and no further copy is made.
first_start=$(docker inspect --format '{{.State.StartedAt}}' "$(new ps -a -q server)")
sleep 1
since=$(date -u +%Y-%m-%dT%H:%M:%SZ)
new start server
check "the second start fails in the same migration" "$(log_holds 'database migration failed')" "yes"
check "that failure is of a second start" "$([ "$(docker inspect --format '{{.State.StartedAt}}' "$(new ps -a -q server)")" != "$first_start" ] && echo second)" "second"
new stop server
check "the second start migrated nothing further" "$(schema)" "$partly"
check "the second start kept the copy and made no other" "$(copies)" "junto_before_$before"
check "the copy has the schema from before the upgrade" "$(sql 'select max(version) from _sqlx_migrations where success' "junto_before_$before")" "$before"
check "the role of the server owns the copy" "$(owners "junto_before_$before")" "junto_server|0"

# The way back, without a name: the script takes the copy that the comment names. The
# copy becomes the database, and the earlier release runs on it. That release connects as
# the first user. Its password is no longer the one of the settings, as after a change of
# the secret in this release, which changes the role of the server only.
sql "alter role junto password 'another'" postgres > /dev/null
epoch_before=$(epoch)
# The way back sets that password in the database postgres. While that database takes no
# change, the statement fails, the way back changes no name, and the log of PostgreSQL holds
# no secret.
sql 'alter database postgres set default_transaction_read_only = on' junto > /dev/null
logged=$(date -u +%Y-%m-%dT%H:%M:%SZ)
status=0
"$work/earlier/deploy/upgrade-undo.sh" local.yaml > "$work/undo.log" 2>&1 || status=$?
sql 'alter database postgres reset default_transaction_read_only' junto > /dev/null
postgres_log "$logged"
check "the way back fails at the statement that sets the password" \
    "$([ "$status" -ne 0 ] && grep -c 'ERROR:  cannot execute ALTER ROLE in a read-only transaction' "$work/postgres.log")" "1"
check "that failure changed no name" "$(copies) $(schema)" "junto_before_$before $partly"
check "the output of that way back holds no secret" "$(secrets_in "$work/undo.log")" "0"
check "the log of PostgreSQL holds no secret after that failure" "$(secrets_in "$work/postgres.log")" "0"
"$work/earlier/deploy/upgrade-undo.sh" local.yaml > "$work/undo.log" 2>&1 || { cat "$work/undo.log"; exit 1; }
check "after the way back no copy is left" "$(copies)" ""
check "the database has the schema from before the upgrade" "$(schema)" "$before"
check "the way back gave the event stream a new epoch" "$([ "$(epoch)" != "$epoch_before" ] && echo new)" "new"
check "the database that the newer version changed is kept, with its comment" "$(comment junto_undone)" \
    "junto: an upgrade of this database has not finished. junto_before_$before is the copy from before the upgrade"
check "the database has no comment" "$(comment junto)" ""
check "the way back states that the earlier release cannot list the directories without a row" \
    "$(grep -c 'did not list the directories that no row of the copy names' "$work/undo.log")" "1"
check "the output of the way back holds no secret" "$(secrets_in "$work/undo.log")" "0"
check "the way back states the role that got the password of the settings" \
    "$(grep -c 'upgrade-undo: the database role junto, as which the server of this checkout connects, has the password of' "$work/undo.log")" "1"
old up -d --build --wait --wait-timeout 600 postgres server
driver check --count 3 || failures=$((failures + 1))
old stop server

# The upgrade. The function that stopped the migration is in the copy too.
sql 'drop function is_inside_call(uuid, uuid)' > /dev/null
# A first start is stopped while PostgreSQL creates the copy (ADR 0018).
new build server > /dev/null 2>&1
stop_during_copy "junto_making_$before"
check "the stopped start left the database of its copy, without a comment" \
    "$(making)|$(comment "junto_making_$before")" "junto_making_$before|"
check "the comment of the database names that database" "$(comment junto)" "junto: the server makes the copy junto_making_$before"
check "the stopped start migrated nothing and left no copy" "$(schema)|$(copies)" "$before|"
since=$(date -u +%Y-%m-%dT%H:%M:%SZ)
new up -d --build --wait --wait-timeout 600 postgres server
after=$(schema)
check "the upgrade applied every migration" "$after" "$newest"
check "the server made a copy before the migration" "$(copies)" "junto_before_$before"
check "the server deleted the database that the stopped start left" "$(making)" ""
check "the log names the copy" "$(log_holds "junto_before_$before")" "yes"
check "the comment of the database no longer states an upgrade that has not finished" "$(comment junto)" ""
check "the server is connected as its role" \
    "$(sql "select string_agg(distinct usename, ' ') from pg_stat_activity where datname = 'junto' and backend_type = 'client backend' and pid <> pg_backend_pid()")" "junto_server"
check "the role owns the database, and every table, sequence and routine of the new schema" "$(owners junto)" "junto_server|0"
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
# This way back gets the stop signal while the server of the checkout lists the directories
# that no row of the copy names, which is after the renames (ADR 0021).
in_front '*" unnamed-files"*' "        kill -TERM \"\$(cat '$work/script')\""
status=$(in_background "$work/undo.log" "$work/earlier/deploy/upgrade-undo.sh" local.yaml "junto_before_$before")
wait_until "the container of the listing has ended" \
    "docker ps -q --filter label=com.docker.compose.project=$project --filter label=com.docker.compose.oneoff=True | wc -l | tr -d ' '" 0
check "a way back that gets the stop signal while it lists the directories ends with the code 143" "$status" "143"
check "its output states that the database is the copy again" \
    "$(grep -c "upgrade-undo: the database is the copy junto_before_$before again" "$work/undo.log")" "1"
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
check "a backup of the earlier release is refused with the two schemas, as the server states them" \
    "$([ "$status" -ne 0 ] && grep -c "the database has the schema $before, and this version of the server needs the schema $after" "$work/restore.log")" "1"
check "the restore states which release restores it" \
    "$(grep -c 'A backup is restored with the checkout of the release that wrote it' "$work/restore.log")" "1"
check "nothing was replaced" "$(grep -c 'failed before anything was replaced. The database and the files are as they were' "$work/restore.log")" "1"
new up -d --build --wait --wait-timeout 600 postgres server
check "the server is the same after the refused restore" \
    "$(curl -fsS --max-time 10 "http://127.0.0.1:$port/api/server" | sed -n 's/.*"server_id":"\([^"]*\)".*/\1/p')" "$identifier"
driver check --count 3 || failures=$((failures + 1))

# A backup with the server stopped, as the upgrade procedure makes it, and its restore. The
# restore deletes the copy from before the migration: it is a copy of the replaced database.
new stop server
./backup.sh local.yaml "$work/backup" > "$work/backup.log" 2>&1 || { cat "$work/backup.log"; exit 1; }
check "a backup with the server stopped has its six files" "$(cd "$work/backup" && LC_ALL=C ls -A | tr '\n' ' ')" "SHA256SUMS database.dump env files.tar left-out schema "
check "its file schema states the schema of the database" "$(cat "$work/backup/schema")" "schema $after"
check "the server is still stopped" "$(new ps --status running --format '{{.Service}}' server)" ""
check "no backup holds deletions back after it" "$(sql 'select count(*) from backups')" "0"
new up -d --build --wait --wait-timeout 600 postgres server
driver send || failures=$((failures + 1))
driver check --count 4 || failures=$((failures + 1))
./restore.sh local.yaml "$work/backup" > "$work/restore.log" 2>&1 || { cat "$work/restore.log"; exit 1; }
check "the restore deleted the copy from before the migration" "$(copies)" ""
new up -d --build --wait --wait-timeout 600 postgres server
driver check --count 3 || failures=$((failures + 1))

# The restore asks the image of the server for its schema, the image that it stages with,
# and not the server that runs. Here the image is the one of the earlier release, which
# does not state a schema, while the server of this release runs.
old build server > /dev/null 2>&1
status=0
./restore.sh local.yaml "$work/backup" > "$work/restore.log" 2>&1 || status=$?
check "a restore with a server image that does not state its schema is refused" \
    "$([ "$status" -ne 0 ] && grep -c 'does not state its schema' "$work/restore.log")" "1"
check "that restore stopped nothing" "$(new ps --status running --format '{{.Service}}' server)" "server"
new build server > /dev/null 2>&1

# What a start of the server leaves that was stopped while PostgreSQL created its copy: a
# database without a comment, which only the comment of the database names. The database is
# the one of the earlier release again, as before an upgrade, so that a migration waits and
# the server makes its copy. A database of such a name that nothing states stays.
new stop server
new exec -T postgres psql -U junto -d postgres -q -v ON_ERROR_STOP=1 -c 'drop database junto with (force)' \
    -c 'create database junto' < /dev/null
new exec -T postgres pg_restore -U junto -d junto --exit-on-error < "$work/earlier-backup/database.dump"
./database-user.sh local.yaml > "$work/user.log" 2>&1 || { cat "$work/user.log"; exit 1; }
new exec -T postgres psql -U junto_server -d postgres -q -v ON_ERROR_STOP=1 \
    -c 'create database junto_making_1 template template0' < /dev/null
stop_during_copy "junto_making_$before"
check "the second stopped start left the database of its copy, without a comment" \
    "$(making)|$(comment "junto_making_$before")" "junto_making_1 junto_making_$before|"
# The restore deletes that database first, while the comment of the database junto names
# it, and the database junto second (ADR 0021). The restore is ended between the two.
in_front '*" --force junto"' "        kill -KILL \"\$PPID\"
        exit 1"
logged=$(date -u +%Y-%m-%dT%H:%M:%SZ)
status=0
PATH="$work/bin:$PATH" ./restore.sh local.yaml "$work/backup" > "$work/restore.log" 2>&1 || status=$?
check "the restore was ended by force" "$status" "137"
check "it had deleted the database that the stopped start left, and not the database junto" \
    "$(making) $(sql "select count(*) from pg_database where datname in ('junto', 'junto_restore')" postgres)" "junto_making_1 2"
check "the database junto is the one from before the restore" "$(schema)" "$before"
./restore.sh local.yaml "$work/backup" > "$work/restore.log" 2>&1 || { cat "$work/restore.log"; exit 1; }
check "the second run of the restore ends" "$(grep -c 'restore: done' "$work/restore.log")" "1"
check "the restored database has the schema of the backup" "$(schema)" "$after"
check "the database of such a name that nothing states is still there, and no other" "$(making)" "junto_making_1"
postgres_log "$logged"
check "the log of PostgreSQL holds no secret after the two restores" "$(secrets_in "$work/postgres.log")" "0"
new up -d --build --wait --wait-timeout 600 postgres server
driver check --count 3 || failures=$((failures + 1))

# A way back inside this release. No later release exists, so the test makes the copy as
# the server makes one, with its name and its comment, and writes a message after it. The
# script of this checkout takes the copy, and the server of this checkout lists the
# directory of that message: no row of the copy names it.
new stop server
new exec -T postgres dropdb -U junto junto_undone < /dev/null
new exec -T postgres psql -U junto_server -d postgres -q -v ON_ERROR_STOP=1 \
    -c "create database junto_before_$after template junto" \
    -c "comment on database junto_before_$after is 'junto: copy of the database junto before a migration'" \
    -c "create database junto_making_$after template template0" \
    -c "comment on database junto is 'junto: the server makes the copy junto_making_$after'" < /dev/null
new up -d --wait --wait-timeout 600 postgres server
driver send || failures=$((failures + 1))
new stop server
written=$({ sql 'select storage_id from attachments' junto; sql 'select storage_id from attachments' "junto_before_$after"; } | LC_ALL=C sort | uniq -u)
./upgrade-undo.sh local.yaml > "$work/undo.log" 2>&1 || { cat "$work/undo.log"; exit 1; }
check "the way back took the copy of this release" "$(copies) $(schema)" " $after"
check "it lists the one directory that was written after the copy, and no other" \
    "$(grep -E '^(unnamed|skip) ' "$work/undo.log")" "skip attachments/$written changed_recently"
check "it explains the directories that were changed in the last 24 hours" \
    "$(grep -c 'upgrade-undo: a line "skip ... changed_recently" names a directory that was changed in the last 24 hours' "$work/undo.log")" "1"
check "it names the database that a stopped start left, and not the one that nothing states" \
    "$(sed -n 's/^upgrade-undo: the database \(junto_making_[0-9]*\) is what a start of the server left.*/\1/p' "$work/undo.log" | tr '\n' ' ')" "junto_making_$after "
new up -d --wait --wait-timeout 600 postgres server
driver check --count 3 || failures=$((failures + 1))
check "a removal leaves the directory while its last change is younger than 24 hours" \
    "$(new exec -T server junto-server unnamed-files --remove < /dev/null)" "skip attachments/$written changed_recently"
new exec -T server touch -d '2 days ago' "/var/lib/junto/attachments/$written" < /dev/null
check "the server removes the directory when its last change is older" \
    "$(new exec -T server junto-server unnamed-files --remove < /dev/null)" "removed attachments/$written"
check "the directory is not on the disk" "$(new exec -T server sh -c 'test -e "$1" && echo exists' sh "/var/lib/junto/attachments/$written" < /dev/null || true)" ""
driver check --count 3 || failures=$((failures + 1))

if [ "$failures" -eq 0 ]; then
    echo "all checks passed"
else
    echo "$failures check(s) failed"
    exit 1
fi
