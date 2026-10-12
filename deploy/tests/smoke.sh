#!/bin/sh
# Starts the local form with new secrets and small limits, waits until every container is
# healthy, asks the health route, and removes the containers, the volumes and the network.
# The sum of the memory limits is under 3 GB. The models of the worker are not installed:
# the worker container runs and waits for jobs.
#
#   deploy/tests/smoke.sh [<project name>]
#
# JUNTO_SMOKE_KEEP=1 leaves the deployment running and prints how to remove it.
# JUNTO_LOCAL_PORT, JUNTO_LOCAL_LIVEKIT_PORT, JUNTO_LOCAL_LIVEKIT_TCP_PORT and
# JUNTO_LOCAL_LIVEKIT_UDP_PORT choose other loopback ports.
set -eu
project=${1:-junto-smoke}
deploy=$(cd "$(dirname "$0")/.." && pwd)
work=$(mktemp -d "${TMPDIR:-/tmp}/$project.XXXXXX")
port=${JUNTO_LOCAL_PORT:-8080}
cd "$deploy"

mkdir "$work/models"
(
    umask 077
    {
        ./generate-secrets.sh
        echo "JUNTO_MODELS_DIR=$work/models"
        echo "JUNTO_LOCAL_PORT=$port"
        echo "JUNTO_LOCAL_LIVEKIT_PORT=${JUNTO_LOCAL_LIVEKIT_PORT:-7880}"
        echo "JUNTO_LOCAL_LIVEKIT_TCP_PORT=${JUNTO_LOCAL_LIVEKIT_TCP_PORT:-7881}"
        echo "JUNTO_LOCAL_LIVEKIT_UDP_PORT=${JUNTO_LOCAL_LIVEKIT_UDP_PORT:-7882}"
        echo "JUNTO_POSTGRES_MEMORY=512m"
        echo "JUNTO_SERVER_MEMORY=512m"
        echo "JUNTO_REDIS_MEMORY=96m"
        echo "JUNTO_LIVEKIT_MEMORY=512m"
        echo "JUNTO_RECORDER_MEMORY=1g"
        echo "JUNTO_WORKER_MEMORY=256m"
    } > "$work/env"
)
compose() {
    docker compose -p "$project" --env-file "$work/env" -f local.yaml "$@"
}

remove() {
    code=$?
    if [ "$code" -ne 0 ]; then
        compose ps -a || true
        compose logs --tail 40 || true
    fi
    if [ "${JUNTO_SMOKE_KEEP:-}" = 1 ] && [ "$code" -eq 0 ]; then
        echo "left running. Settings: $work/env. Remove with:"
        echo "  cd $deploy && docker compose -p $project --env-file $work/env -f local.yaml down -v && rm -rf $work"
    else
        compose down -v --timeout 20 > /dev/null 2>&1 || true
        rm -rf "$work"
    fi
    exit "$code"
}
trap remove EXIT
trap 'exit 130' INT TERM

compose up -d --build --wait --wait-timeout 300

body=$(curl -fsS --max-time 10 "http://127.0.0.1:$port/api/v1/health")
case "$body" in
    *'"database":true'*) echo "ok    the health route answers 200 and reaches the database" ;;
    *) echo "FAIL  health answer: $body"; exit 1 ;;
esac

# The admission check reaches the media server within its interval of 2 seconds.
tries=0
until curl -fsS --max-time 10 "http://127.0.0.1:$port/api/v1/health" | grep -q '"media_server":true'; do
    tries=$((tries + 1))
    if [ "$tries" -ge 30 ]; then
        echo "FAIL  the server does not reach the media server"
        exit 1
    fi
    sleep 1
done
echo "ok    the server reaches the media server"

if compose logs --no-log-prefix server 2>/dev/null | grep -q "This server has no owner"; then
    echo "ok    the first start printed an owner code"
else
    echo "FAIL  the server printed no owner code"
    exit 1
fi

# The database users (ADR 0022).
expect() {
    if [ "$2" = "$3" ]; then
        echo "ok    $1"
    else
        echo "FAIL  $1: got \"$2\", expected \"$3\""
        exit 1
    fi
}
sql() {
    compose exec -T postgres psql -U junto -d junto -At -c "$1" < /dev/null
}
# A login over the network of the container, with the password of the settings.
over_network() {
    compose exec -T postgres sh -c 'PGPASSWORD="$POSTGRES_PASSWORD" psql -h "$(hostname)" -U "$1" -d junto -At -c "select current_user" 2>&1' sh "$1" < /dev/null || true
}
expect "the role of the server may log in and create a database, has no other attribute and is in no other role" \
    "$(sql "select rolcanlogin, rolcreatedb, rolsuper, rolcreaterole, rolreplication, rolbypassrls, (select count(*) from pg_auth_members where member = r.oid) from pg_roles r where rolname = 'junto_server'")" \
    "t|t|f|f|f|f|0"
expect "the role of the server owns the database" "$(sql "select datdba::regrole from pg_database where datname = 'junto'")" "junto_server"
expect "the role of the server owns every table, sequence and routine of the database" \
    "$(sql "select (select count(*) from pg_class where relnamespace = 'public'::regnamespace and relowner = r) > 0, (select count(*) from pg_class where relnamespace = 'public'::regnamespace and relowner <> r) + (select count(*) from pg_proc where pronamespace = 'public'::regnamespace and proowner <> r) from (select 'junto_server'::regrole::oid as r) role")" \
    "t|0"
expect "the server is connected as that role" \
    "$(sql "select string_agg(distinct usename, ' ') from pg_stat_activity where datname = 'junto' and backend_type = 'client backend' and pid <> pg_backend_pid()")" \
    "junto_server"
expect "PostgreSQL reads the access rules of the repository" "$(sql 'show hba_file')" "/etc/junto/pg_hba.conf"
expect "the role of the server logs in over the network with the password" "$(over_network junto_server)" "junto_server"
case "$(over_network junto)" in
    *"no pg_hba.conf entry"*) echo "ok    the first user of the database server has no path over the network" ;;
    *) echo "FAIL  the first user of the database server over the network: $(over_network junto)"; exit 1 ;;
esac

for service in postgres server redis livekit recorder worker; do
    state=$(compose ps --format '{{.State}}' "$service")
    if [ "$state" = running ]; then
        echo "ok    $service runs"
    else
        echo "FAIL  $service is $state"
        exit 1
    fi
done
echo "all checks passed"
