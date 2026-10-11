#!/bin/sh
# Checks the Compose files of every form without starting a container (ADR 0015):
# - each form is valid with exactly the settings that it needs, and reads no unset variable;
# - without any one of those settings Compose stops and names the variable;
# - the rendered configuration follows the rules of check_rendered.py: published ports,
#   pinned images, users, capabilities, limits, networks.
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

if [ "$failures" -eq 0 ]; then
    echo "all checks passed"
else
    echo "$failures check(s) failed"
    exit 1
fi
