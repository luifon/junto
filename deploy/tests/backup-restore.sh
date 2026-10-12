#!/bin/sh
# Tests deploy/backup.sh and deploy/restore.sh with the local form (ADR 0902, "a restore
# from a backup"; ADR 0911):
# - a backup of a server that runs, while messages with attachments are written, with a
#   channel and a recording that are deleted between the copy of the database and the list
#   of the files;
# - restores that are refused, and one that fails before anything is replaced;
# - a restore into a new, empty deployment with other secrets, which is ended by force after
#   the files were replaced, and its second run;
# - in the restored deployment: every message, attachment and recording directory that the
#   database names, and nothing of what was deleted before the backup.
# Only the database and the server are started. Everything is removed at the end. The test
# waits three times for the deletion of files by the server and takes about 10 minutes.
#
#   deploy/tests/backup-restore.sh [<project name>]
set -eu
project=${1:-junto-backup-test}
deploy=$(cd "$(dirname "$0")/.." && pwd)
work=$(mktemp -d "${TMPDIR:-/tmp}/$project.XXXXXX")
port=${JUNTO_LOCAL_PORT:-8080}
cd "$deploy"

mkdir "$work/models" "$work/bin"
settings() {
    (
        umask 077
        {
            ./generate-secrets.sh
            echo "JUNTO_MODELS_DIR=$work/models"
            echo "JUNTO_LOCAL_PORT=$port"
            echo "JUNTO_POSTGRES_MEMORY=512m"
            echo "JUNTO_SERVER_MEMORY=512m"
        } > "$1"
    )
}
settings "$work/env"
export JUNTO_ENV_FILE="$work/env" COMPOSE_PROJECT_NAME="$project"
compose() {
    docker compose --env-file "$work/env" -f local.yaml "$@"
}
driver() {
    step=$1
    shift
    python3 -B "$deploy/tests/content_driver.py" "$step" --project "$project" --env-file "$work/env" \
        --compose-file local.yaml --origin "http://127.0.0.1:$port" --state "$work/state.json" \
        --stop-file "$work/stop" "$@"
}
writer=""
remove() {
    code=$?
    [ -z "$writer" ] || kill "$writer" 2>/dev/null || true
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
sql() {
    compose exec -T postgres psql -U junto -d postgres -At -c "$1" < /dev/null
}
checksums() {
    (cd "$1" && shift && { sha256sum "$@" 2>/dev/null || shasum -a 256 "$@"; } > SHA256SUMS)
}
# The values of the secrets of a settings file that a log holds.
secrets_in() {
    found=0
    for value in $(sed -n -E 's/^JUNTO_[A-Z_]*(PASSWORD|SECRET|API_KEY)=//p' "$1"); do
        if grep -q -F "$value" "$2"; then
            found=$((found + 1))
        fi
    done
    echo "$found"
}
# A program named "docker" in front of the real one, for the script under test. It runs a
# step of the test before or after the one call whose arguments hold a text (ADR 0911).
real=$(command -v docker)
in_front() {
    cat > "$work/bin/docker" <<EOF
#!/bin/sh
case "\$*" in
    *"$1"*)
$2
        ;;
esac
exec "$real" "\$@"
EOF
    chmod +x "$work/bin/docker"
}

compose up -d --build --wait --wait-timeout 600 postgres server
first_id=$(server_id)
driver prepare

# The backup. Messages are written during all of it. Before the server lists the files,
# and so after the copy of the database, the test deletes a channel and a recording and
# waits longer than the server waits before it deletes their directories.
driver write &
writer=$!
tries=0
until [ "$(wc -l < "$work/stop.sent" 2>/dev/null || echo 0)" -ge 3 ]; do
    tries=$((tries + 1))
    if [ "$tries" -ge 60 ]; then
        echo "FAIL  no message was written before the backup"
        exit 1
    fi
    sleep 1
done
# backup.sh asks for the list of files twice; the step of the test runs before the first.
in_front " backup-files " "        if [ ! -e '$work/between' ]; then
            touch '$work/between'
            python3 -B '$deploy/tests/content_driver.py' between --project '$project' --env-file '$work/env' --compose-file local.yaml --origin 'http://127.0.0.1:$port' --state '$work/state.json' >&2 || exit 1
        fi"
started=$(date +%s)
server_started=$(docker inspect --format '{{.State.StartedAt}}' "$(compose ps -q server)")
PATH="$work/bin:$PATH" ./backup.sh local.yaml "$work/backup" > "$work/backup.log" 2>&1 || { cat "$work/backup.log"; exit 1; }
touch "$work/stop"
written=0
wait "$writer" || written=$?
writer=""
cat "$work/backup.log"
check "the backup has its five files" "$(cd "$work/backup" && LC_ALL=C ls -A | tr '\n' ' ')" "SHA256SUMS database.dump env files.tar left-out "
check "the server ran during the backup and runs after it" "$(compose ps --status running --format '{{.Service}}' server)" "server"
check "the server was not stopped for the backup" "$(docker inspect --format '{{.State.StartedAt}}' "$(compose ps -q server)")" "$server_started"
check "the process that wrote messages during the backup ended without a failure" "$written" "0"
check "the backup is readable for its owner only" \
    "$(find "$work/backup" \( -perm -040 -o -perm -020 -o -perm -010 -o -perm -004 -o -perm -002 -o -perm -001 \) | wc -l | tr -d ' ')" "0"
check "the output of the backup holds no secret" "$(secrets_in "$work/env" "$work/backup.log")" "0"
check "the backup states the directory that is not on the disk" \
    "$(grep -c '^backup: 1 directories that the database names are not on the disk' "$work/backup.log")" "1"
driver released --backup "$work/backup" || failures=$((failures + 1))

# A backup whose archive ends inside a file is refused before a container is stopped.
cp -R "$work/backup" "$work/cut-archive"
head -c 100000 "$work/backup/files.tar" > "$work/cut-archive/files.tar"
checksums "$work/cut-archive" database.dump files.tar env left-out
status=0
./restore.sh local.yaml "$work/cut-archive" > "$work/restore.log" 2>&1 || status=$?
check "a restore of an archive that was cut is refused" "$([ "$status" -ne 0 ] && grep -c 'is not a readable archive' "$work/restore.log")" "1"
check "that restore stopped nothing" "$(compose ps --status running --format '{{.Service}}' server)" "server"

# An archive whose members have other names than the two directories is refused.
cp -R "$work/backup" "$work/renamed"
mkdir "$work/renamed/content"
tar -xf "$work/backup/files.tar" -C "$work/renamed/content" 2>/dev/null || true
(cd "$work/renamed/content" && tar -cf ../files.tar ./attachments ./recordings)
rm -rf "$work/renamed/content"
checksums "$work/renamed" database.dump files.tar env left-out
status=0
./restore.sh local.yaml "$work/renamed" > "$work/restore.log" 2>&1 || status=$?
check "a restore of an archive with other member names is refused" "$([ "$status" -ne 0 ] && grep -c 'has a member outside' "$work/restore.log")" "1"

# An archive with a member that is a link is refused.
cp -R "$work/backup" "$work/linked"
mkdir -p "$work/linked/content/attachments" "$work/linked/content/recordings"
ln -s /etc "$work/linked/content/attachments/outside"
(cd "$work/linked/content" && tar -cf ../files.tar attachments recordings)
rm -rf "$work/linked/content"
checksums "$work/linked" database.dump files.tar env left-out
status=0
./restore.sh local.yaml "$work/linked" > "$work/restore.log" 2>&1 || status=$?
check "a restore of an archive with a link is refused" "$([ "$status" -ne 0 ] && grep -c 'that is not a file or a directory' "$work/restore.log")" "1"
check "that restore stopped nothing" "$(compose ps --status running --format '{{.Service}}' server)" "server"

# A list of checksums that does not name the archive is refused.
cp -R "$work/backup" "$work/unlisted"
checksums "$work/unlisted" database.dump env left-out
cp "$work/unlisted/files.tar" "$work/unlisted/filesXtar"
(cd "$work/unlisted" && { sha256sum filesXtar 2>/dev/null || shasum -a 256 filesXtar; } >> SHA256SUMS)
status=0
./restore.sh local.yaml "$work/unlisted" > "$work/restore.log" 2>&1 || status=$?
check "a restore without a checksum of the archive is refused" "$([ "$status" -ne 0 ] && grep -c 'has no checksum of files.tar' "$work/restore.log")" "1"

# A backup whose copy of the database was cut has the right checksums and fails while it is
# staged: nothing is replaced, and the staged parts are removed.
cp -R "$work/backup" "$work/cut-database"
head -c 20000 "$work/backup/database.dump" > "$work/cut-database/database.dump"
checksums "$work/cut-database" database.dump files.tar env left-out
compose exec -T server sh -c 'echo newer > /var/lib/junto/attachments/after-the-backup' < /dev/null
status=0
./restore.sh local.yaml "$work/cut-database" > "$work/restore.log" 2>&1 || status=$?
check "a restore of a copy of the database that was cut fails before anything is replaced" \
    "$([ "$status" -ne 0 ] && grep -c 'failed before anything was replaced. The database and the files are as they were' "$work/restore.log")" "1"
check "the output of that restore holds no secret" "$(secrets_in "$work/env" "$work/restore.log")" "0"
check "no staged database is left" "$(sql "select count(*) from pg_database where datname = 'junto_restore'")" "0"
compose up -d --wait --wait-timeout 300 postgres server
check "after the failed restore the server is the same" "$(server_id)" "$first_id"
check "after the failed restore the files are as they were" \
    "$(compose exec -T server sh -c 'cat /var/lib/junto/attachments/after-the-backup; ls -A /var/lib/junto/attachments | grep -c restore' < /dev/null | tr '\n' ' ')" \
    "newer 0 "

# A new, empty deployment with other secrets, as on a new host.
compose down -v --timeout 20
settings "$work/env"
compose up -d --wait --wait-timeout 300 postgres server
second_id=$(server_id)
check "the new deployment is another server" "$([ -n "$second_id" ] && [ "$second_id" != "$first_id" ] && echo other)" "other"
# What the other server left: an export, a transcription job in the spool, and a partial
# file of the recorder.
compose exec -T server sh -c 'echo export > /var/lib/junto/exports/of-the-other-server; mkdir /var/lib/junto/spool/of-the-other-server' < /dev/null
compose run --rm --no-deps -T --entrypoint sh recorder -c 'echo audio > /home/egress/tmp/of-the-other-server' < /dev/null

# The restore is ended by force when the files were replaced and the database has not got
# its name yet.
cp "$work/backup/env" "$work/env"
in_front "find /var/lib/junto/exports" "        '$real' \"\$@\"
        kill -KILL \"\$PPID\"
        exit 1"
status=0
PATH="$work/bin:$PATH" ./restore.sh local.yaml "$work/backup" > "$work/restore.log" 2>&1 || status=$?
check "the restore was ended by force" "$status" "137"
check "the restore that was ended did not finish" "$(grep -c 'restore: done' "$work/restore.log" || true)" "0"
check "after it, the staged files have taken their places and the exports are deleted" \
    "$(compose run --rm --no-deps -T --entrypoint sh server -c 'ls -A /var/lib/junto/attachments | grep -c restore; ls -A /var/lib/junto/exports | wc -l' < /dev/null 2>/dev/null | tr '\n' ' ')" \
    "0 0 "
check "after it, no database has the name junto" "$(sql "select count(*) from pg_database where datname = 'junto'")" "0"
compose up -d postgres server
tries=0
until compose logs --no-log-prefix server 2>/dev/null | grep -q 'database "junto" does not exist'; do
    tries=$((tries + 1))
    if [ "$tries" -ge 60 ]; then
        break
    fi
    sleep 2
done
check "a server that is started then does not start, and its log names the database" \
    "$(compose logs --no-log-prefix server 2>/dev/null | grep -c -m 1 'database "junto" does not exist')" "1"
check "that server is not healthy" "$(docker inspect --format '{{.State.Health.Status}}' "$(compose ps -a -q server)" | grep -c '^healthy$' || true)" "0"

# The second run restores everything again.
./restore.sh local.yaml "$work/backup" > "$work/restore.log" 2>&1 || { cat "$work/restore.log"; exit 1; }
check "the second run of the restore ends" "$(grep -c 'restore: done' "$work/restore.log")" "1"
check "the output of the restore holds no secret" "$(secrets_in "$work/env" "$work/restore.log")" "0"
compose up -d --wait --wait-timeout 300 postgres server
check "after the restore the server has the identifier of the backup" "$(server_id)" "$first_id"
check "the restore emptied the spool of transcription jobs" "$(compose exec -T server sh -c 'cd /var/lib/junto/spool && ls -A | wc -l' < /dev/null | tr -d ' ')" "0"
check "the restore emptied the temporary directory of the recorder" \
    "$(compose run --rm --no-deps -T --entrypoint sh recorder -c 'cd /home/egress/tmp && ls -A | wc -l' < /dev/null 2>/dev/null | tr -d ' ')" "0"
driver verify --set "backup_started=$started" || failures=$((failures + 1))

# A backup that expired while its files were copied is not a backup: the server deleted
# files again. The test ends the backup in the database after the archive was written.
in_front "--verbatim-files-from" "        code=0
        '$real' \"\$@\" || code=\$?
        '$real' compose --env-file '$work/env' -f local.yaml exec -T postgres psql -U junto -d junto -q -c \"update backups set started_at = now() - interval '2 days', expires_at = now() - interval '1 day'\" < /dev/null > /dev/null 2>&1
        exit \$code"
status=0
PATH="$work/bin:$PATH" ./backup.sh local.yaml "$work/expired" > "$work/backup.log" 2>&1 || status=$?
check "a backup that expired during the copy fails" "$([ "$status" -ne 0 ] && grep -c 'backup: the backup expired' "$work/backup.log")" "1"
check "it leaves no directory" "$([ -e "$work/expired" ] && echo exists)" ""

# A complete attachment whose directory is not on the disk: the backup is written, names
# the directory, and ends with the code 4.
lost=$(compose exec -T postgres psql -U junto -d junto -At -c "select storage_id from attachments where complete and removed_at is null order by id limit 1" < /dev/null)
compose exec -T server rm -rf "/var/lib/junto/attachments/$lost" < /dev/null
status=0
./backup.sh local.yaml "$work/incomplete" > "$work/backup.log" 2>&1 || status=$?
check "a backup without the directory of a complete attachment ends with the code 4" "$status" "4"
check "that backup names the directory" "$(grep -c "^absent attachments/$lost\$" "$work/incomplete/left-out" 2>/dev/null || true)" "1"
check "that backup has its five files with the right checksums" \
    "$(cd "$work/incomplete" && LC_ALL=C ls -A | tr '\n' ' ' && { sha256sum -c SHA256SUMS 2>/dev/null || shasum -a 256 -c SHA256SUMS; } | grep -c ': OK$')" \
    "SHA256SUMS database.dump env files.tar left-out 4"
check "no backup holds deletions back after it" "$(compose exec -T postgres psql -U junto -d junto -At -c 'select count(*) from backups' < /dev/null)" "0"

if [ "$failures" -eq 0 ]; then
    echo "all checks passed"
else
    echo "$failures check(s) failed"
    exit 1
fi
