#!/bin/sh
# Compares the CPU limits of a form with the CPUs of the host (docs/install.md,
# "Requirements"; ADR 0015). Docker creates no container whose CPU limit is above the number
# of CPUs of its host, so a default that fits a large host stops the start on a small one.
# Run it when deploy/.env is complete, before the first start.
#
#   deploy/check-limits.sh <form file> [<number of CPUs>]
#
# Without the number, the script asks the Docker daemon for the CPUs of this host. With the
# number, it checks the settings for a host of that size, on any machine.
#
# JUNTO_ENV_FILE names another settings file than deploy/.env.
#
# Exit codes: 0 when every limit fits; 1 when one does not, with one line for each setting;
# 2 when the form or the settings cannot be read.
set -eu

if [ "$#" -lt 1 ] || [ "$#" -gt 2 ]; then
    echo "usage: $0 <form file> [<number of CPUs>]" >&2
    exit 2
fi
deploy=$(cd "$(dirname "$0")" && pwd)
form=$(basename "$1")
settings=${JUNTO_ENV_FILE:-$deploy/.env}
cd "$deploy"

if [ ! -f "$form" ] || [ ! -f "$settings" ]; then
    echo "check-limits: $deploy/$form or $settings does not exist" >&2
    exit 2
fi
if [ "$#" -eq 2 ]; then
    cpus=$2
else
    cpus=$(docker info --format '{{.NCPU}}') || cpus=""
fi
case "$cpus" in
    "" | 0 | *[!0-9]*)
        echo "check-limits: the number of CPUs of the host is not known (\"$cpus\"). Name it as the second argument." >&2
        exit 2
        ;;
esac

# Compose prints the configuration with every setting applied: each service at two spaces
# below "services:", and its CPU limit as "cpus:" at four spaces.
if ! rendered=$(docker compose --env-file "$settings" -f "$form" config); then
    echo "check-limits: $form cannot be read with $settings" >&2
    exit 2
fi
printf '%s\n' "$rendered" | awk -v host="$cpus" '
    /^[^ ]/ { in_services = ($0 == "services:"); next }
    in_services && /^  [^ ]/ { service = $1; sub(/:$/, "", service); next }
    in_services && /^    cpus: / {
        limit = $2
        gsub(/"/, "", limit)
        if (limit + 0 > host + 0) {
            setting = toupper(service)
            gsub(/-/, "_", setting)
            printf "check-limits: the container %s has the CPU limit %s, and the host has %d. Docker does not create it. Set JUNTO_%s_CPUS=%d in deploy/.env.\n", service, limit, host, setting, host
            above = 1
        }
    }
    END { exit above }
'
