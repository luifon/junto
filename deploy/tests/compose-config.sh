#!/bin/sh
# Checks the Compose files of every form without starting a container (ADR 0015):
# - each form is valid with exactly the settings that it needs, and reads no unset variable;
# - without any one of those settings Compose stops and names the variable;
# - the rendered configuration follows the rules of check_rendered.py: published ports,
#   pinned images, users, capabilities, limits, networks;
# - every "docker run" of the scripts and of the install document names its limits;
# - check-limits.sh names the CPU limits that a host with fewer CPUs refuses.
#
#   deploy/tests/compose-config.sh
set -eu
deploy=$(cd "$(dirname "$0")/.." && pwd)
work=$(mktemp -d "${TMPDIR:-/tmp}/junto-compose-config.XXXXXX")
trap 'rm -rf "$work"' EXIT
cd "$deploy"

./generate-secrets.sh | grep -v '^#' > "$work/secrets"
value() {
    grep "^$1=" env.example || { echo "env.example has no line for $1" >&2; exit 1; }
}
needed() {
    case "$1" in
        local) echo "JUNTO_POSTGRES_PASSWORD JUNTO_LIVEKIT_API_KEY JUNTO_LIVEKIT_API_SECRET JUNTO_REDIS_PASSWORD JUNTO_MODELS_DIR" ;;
        one-host) echo "JUNTO_POSTGRES_PASSWORD JUNTO_LIVEKIT_API_KEY JUNTO_LIVEKIT_API_SECRET JUNTO_REDIS_PASSWORD JUNTO_MODELS_DIR JUNTO_SERVER_HOST JUNTO_LIVEKIT_HOST JUNTO_TURN_HOST JUNTO_PUBLIC_IP JUNTO_ACME_EMAIL" ;;
        two-hosts-data) echo "JUNTO_POSTGRES_PASSWORD JUNTO_LIVEKIT_API_KEY JUNTO_LIVEKIT_API_SECRET JUNTO_REDIS_PASSWORD JUNTO_MODELS_DIR JUNTO_SERVER_HOST JUNTO_LIVEKIT_HOST JUNTO_PUBLIC_IP" ;;
        two-hosts-public) echo "JUNTO_LIVEKIT_API_KEY JUNTO_LIVEKIT_API_SECRET JUNTO_REDIS_PASSWORD JUNTO_SERVER_HOST JUNTO_LIVEKIT_HOST JUNTO_TURN_HOST JUNTO_PUBLIC_IP JUNTO_ACME_EMAIL" ;;
    esac
}
# Writes the settings of a form to $work/env, without the variable $2.
settings() {
    : > "$work/env"
    for setting in $(needed "$1"); do
        [ "$setting" != "${2:-}" ] || continue
        case "$setting" in
            JUNTO_POSTGRES_PASSWORD | JUNTO_LIVEKIT_API_KEY | JUNTO_LIVEKIT_API_SECRET | JUNTO_REDIS_PASSWORD)
                grep "^$setting=" "$work/secrets" >> "$work/env" ;;
            *) value "$setting" >> "$work/env" ;;
        esac
    done
}

failures=0
fail() {
    echo "FAIL  $1"
    failures=$((failures + 1))
}

for form in local one-host two-hosts-data two-hosts-public; do
    settings "$form"
    if ! docker compose --env-file "$work/env" -f "$form.yaml" config --format json > "$work/$form.json" 2> "$work/errors"; then
        cat "$work/errors"
        fail "$form.yaml is not valid with its settings"
        continue
    fi
    if [ -s "$work/errors" ]; then
        cat "$work/errors"
        fail "$form.yaml reads a variable that its settings do not have"
    elif python3 "$deploy/tests/check_rendered.py" "$form" "$work/$form.json"; then
        echo "ok    $form.yaml is valid with its settings and follows the rules"
    else
        fail "$form.yaml breaks a rule"
    fi
    for name in $(needed "$form"); do
        settings "$form" "$name"
        if docker compose --env-file "$work/env" -f "$form.yaml" config -q > /dev/null 2> "$work/errors"; then
            fail "$form.yaml is valid without $name"
        elif grep -q "required variable $name is missing a value" "$work/errors"; then
            echo "ok    $form.yaml without $name stops and names the variable"
        else
            cat "$work/errors"
            fail "$form.yaml without $name does not name the variable"
        fi
    done
done

settings one-host
if docker compose --env-file "$work/env" -f one-host.yaml -f worker-gpu.yaml config -q 2> "$work/errors"; then
    echo "ok    worker-gpu.yaml is valid with one-host.yaml"
else
    cat "$work/errors"
    fail "worker-gpu.yaml is not valid"
fi

if grep -E '^[A-Z_]+=' env.example | grep -E '(PASSWORD|SECRET|API_KEY)='; then
    fail "env.example sets a secret"
else
    echo "ok    env.example sets no secret"
fi

# A container that "docker run" starts has no service of a Compose file, so its command
# names its limits (ADR 0015).
if python3 "$deploy/tests/check_run_limits.py" "$deploy"/*.sh "$deploy/../docs/install.md"; then
    echo "ok    every \"docker run\" of the scripts and of the install document names a memory limit, a CPU limit and a limit of processes"
else
    fail "a \"docker run\" of the deployment names no limit"
fi

# Docker creates no container whose CPU limit is above the CPUs of its host. check-limits.sh
# names each such setting. On a host with one CPU these are the settings whose default is
# above 1; the defaults are read from env.example, so the three places agree. A CPU setting
# of the environment of this script would replace the one of the settings file.
for setting in $(env | sed -n 's/^\(JUNTO_[A-Z]*_CPUS\)=.*/\1/p'); do
    unset "$setting"
done
host_cpus=$(docker info --format '{{.NCPU}}')
for form in local one-host two-hosts-data two-hosts-public; do
    settings "$form"
    services=$(docker compose --env-file "$work/env" -f "$form.yaml" config --services)
    expected=$(sed -n 's/^#\(JUNTO_\([A-Z]*\)_CPUS\)=\([0-9.]*\)$/\1 \2 \3/p' env.example | while read -r setting service limit; do
        service=$(printf '%s' "$service" | tr '[:upper:]' '[:lower:]')
        if printf '%s\n' "$services" | grep -q -x "$service" && awk -v limit="$limit" 'BEGIN { exit !(limit > 1) }'; then
            echo "$setting"
        fi
    done | LC_ALL=C sort | tr '\n' ' ')
    status=0
    JUNTO_ENV_FILE="$work/env" ./check-limits.sh "$form.yaml" 1 > "$work/limits" 2> "$work/errors" || status=$?
    named=$(sed -n 's/.* Set \(JUNTO_[A-Z_]*_CPUS\)=1 in .*/\1/p' "$work/limits" | LC_ALL=C sort | tr '\n' ' ')
    if [ "$status" -eq 1 ] && [ -n "$expected" ] && [ "$named" = "$expected" ]; then
        echo "ok    $form.yaml on a host with one CPU: check-limits.sh names the settings whose default is above 1"
    else
        cat "$work/limits" "$work/errors"
        fail "$form.yaml on a host with one CPU: check-limits.sh ended with $status and named \"$named\", expected \"$expected\""
    fi
    for setting in $named; do
        echo "$setting=1" >> "$work/env"
    done
    if JUNTO_ENV_FILE="$work/env" ./check-limits.sh "$form.yaml" 1 > "$work/limits" 2>&1 && [ ! -s "$work/limits" ]; then
        echo "ok    $form.yaml with those settings at 1 fits a host with one CPU"
    else
        cat "$work/limits"
        fail "$form.yaml with every named setting at 1 does not fit a host with one CPU"
    fi
    settings "$form"
    asked=0
    JUNTO_ENV_FILE="$work/env" ./check-limits.sh "$form.yaml" > "$work/asked" 2>&1 || asked=$?
    stated=0
    JUNTO_ENV_FILE="$work/env" ./check-limits.sh "$form.yaml" "$host_cpus" > "$work/stated" 2>&1 || stated=$?
    if [ "$asked" -le 1 ] && [ "$asked" -eq "$stated" ] && cmp -s "$work/asked" "$work/stated"; then
        echo "ok    $form.yaml: without a number, check-limits.sh takes the CPUs of the Docker host"
    else
        cat "$work/asked"
        fail "$form.yaml: check-limits.sh without a number does not answer as with the $host_cpus CPUs of the Docker host"
    fi
done

if [ "$failures" -eq 0 ]; then
    echo "all checks passed"
else
    echo "$failures check(s) failed"
    exit 1
fi
