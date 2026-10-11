#!/bin/sh
# Tests deploy/backup.sh and deploy/restore.sh with the local form (ADR 0902, "a restore
# from a backup"): a backup of a server with files, a restore that fails before anything is
# replaced, and a restore into a new, empty deployment whose database has another password.
# The identifier of the server and the files must be those of the backup afterwards.
# Everything is removed at the end. The sum of the memory limits is under 3 GB.
#
#   deploy/tests/backup-restore.sh [<project name>]
set -eu
project=${1:-junto-backup-test}
deploy=$(cd "$(dirname "$0")/.." && pwd)
work=$(mktemp -d "${TMPDIR:-/tmp}/$project.XXXXXX")
port=${JUNTO_LOCAL_PORT:-8080}
cd "$deploy"

mkdir "$work/models"
settings() {
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
        } > "$1"
    )
}
settings "$work/env"
export JUNTO_ENV_FILE="$work/env" COMPOSE_PROJECT_NAME="$project"
compose() {
    docker compose --env-file "$work/env" -f local.yaml "$@"
}
remove() {
    code=$?
    if [ "$code" -ne 0 ]; then
        compose ps -a || true
        compose logs --tail 30 server postgres || true
    fi
    compose down -v --timeout 20 > /dev/null 2>&1 || true
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
server_id() {
    curl -fsS --max-time 10 "http://127.0.0.1:$port/api/server" | sed -n 's/.*"server_id":"\([^"]*\)".*/\1/p'
}
files() {
    compose exec -T server sh -c 'cat /var/lib/junto/attachments/mark /var/lib/junto/recordings/call/mark 2>&1; sha256sum < /var/lib/junto/attachments/large; LC_ALL=C ls -A /var/lib/junto/attachments /var/lib/junto/recordings | tr "\n" " "'
}

compose up -d --build --wait --wait-timeout 300
first_id=$(server_id)
compose exec -T server sh -c 'echo attachment-of-the-backup > /var/lib/junto/attachments/mark; head -c 200000 /dev/urandom > /var/lib/junto/attachments/large; mkdir /var/lib/junto/recordings/call && echo recording-of-the-backup > /var/lib/junto/recordings/call/mark'
expected=$(files)

./backup.sh local.yaml "$work/backup"
check "the backup has its four files" "$(cd "$work/backup" && LC_ALL=C ls | tr '\n' ' ')" "SHA256SUMS database.dump env files.tar "
check "the server runs again after the backup" "$(compose ps --status running --format '{{.Service}}' server)" "server"
check "the backup is readable for its owner only" "$(find "$work/backup" -perm -004 | wc -l | tr -d ' ')" "0"

# A backup whose archive ends inside a file has the right checksums, can be listed up to
# that file, and cannot be unpacked whole.
cp -R "$work/backup" "$work/damaged"
head -c 100000 "$work/backup/files.tar" > "$work/damaged/files.tar"
(cd "$work/damaged" && { sha256sum database.dump files.tar env 2>/dev/null || shasum -a 256 database.dump files.tar env; } > SHA256SUMS)
compose exec -T server sh -c 'echo newer > /var/lib/junto/attachments/after-the-backup'
status=0
./restore.sh local.yaml "$work/damaged" > "$work/restore.log" 2>&1 || status=$?
check "a restore of a damaged backup fails" "$([ "$status" -ne 0 ] && echo failed)" "failed"
compose up -d --wait --wait-timeout 300
check "after the failed restore the server is the same" "$(server_id)" "$first_id"
check "after the failed restore the files are as they were" \
    "$(compose exec -T server sh -c 'cat /var/lib/junto/attachments/mark /var/lib/junto/attachments/after-the-backup; LC_ALL=C ls -A /var/lib/junto/attachments | tr "\n" " "')" \
    "attachment-of-the-backup
newer
after-the-backup large mark "

# An archive whose members have other names than the two directories is refused.
cp -R "$work/backup" "$work/renamed"
mkdir "$work/renamed/content"
tar -xf "$work/backup/files.tar" -C "$work/renamed/content" 2>/dev/null || true
(cd "$work/renamed/content" && tar -cf ../files.tar ./attachments ./recordings)
rm -rf "$work/renamed/content"
(cd "$work/renamed" && { sha256sum database.dump files.tar env 2>/dev/null || shasum -a 256 database.dump files.tar env; } > SHA256SUMS)
status=0
./restore.sh local.yaml "$work/renamed" > "$work/restore.log" 2>&1 || status=$?
check "a restore of an archive with other member names fails" "$([ "$status" -ne 0 ] && grep -c 'has a member outside' "$work/restore.log")" "1"
check "that restore stopped nothing" "$(compose ps --status running --format '{{.Service}}' server)" "server"

# A new, empty deployment with other secrets, as on a new host.
compose down -v --timeout 20
settings "$work/env"
compose up -d --wait --wait-timeout 300
second_id=$(server_id)
check "the new deployment is another server" "$([ -n "$second_id" ] && [ "$second_id" != "$first_id" ] && echo other)" "other"

cp "$work/backup/env" "$work/env"
./restore.sh local.yaml "$work/backup"
compose up -d --wait --wait-timeout 300
check "after the restore the server has the identifier of the backup" "$(server_id)" "$first_id"
check "after the restore the files are those of the backup" "$(files)" "$expected"

if [ "$failures" -eq 0 ]; then
    echo "all checks passed"
else
    echo "$failures check(s) failed"
    exit 1
fi
