#!/bin/sh
# Runs the CI jobs of a commit on a Linux host over SSH and on this macOS machine, and prints
# their results (ADR 0905).
#
#   ci.sh run <commit> [job...]     sends the commit to the Linux host and starts its jobs
#                                   there, detached: the run continues when this machine
#                                   sleeps or the connection drops. Prints the run name.
#   ci.sh local <commit> [job...]   runs the macOS jobs here, through tools/serial.sh.
#   ci.sh status <commit or run>    prints each job of a run: passed, failed, running or
#                                   waiting, with its time, and copies the log of a failed
#                                   job to target/ci-logs/<run>/. For a commit: its latest
#                                   run on each runner, and whether the commit passed.
#   ci.sh forget <run>              removes a run, its ref and, when no other ref keeps it,
#                                   its commit from the runner that has it.
#
# The SSH alias of the Linux host and its CI directory come from JUNTO_CI_HOST and
# JUNTO_CI_DIR, in the environment or in .env.local of the repository. The CI directory of
# this machine is JUNTO_CI_LOCAL_DIR (default ~/.cache/junto-ci).
set -eu
root=$(CDPATH='' cd -- "$(dirname -- "$0")/../.." && pwd)

# The jobs of each runner. A commit has passed CI when its latest run on each runner has
# finished with every job of this list passed.
linux_jobs="rust-linux licences transcription-worker layout-page"
macos_jobs="rust-macos deployment"

# .env.local of this working tree, or else of the main working tree of the repository.
env_file=$root/.env.local
if [ ! -f "$env_file" ]; then
    env_file=$(git -C "$root" rev-parse --path-format=absolute --git-common-dir)/../.env.local
fi
setting() {
    eval "value=\${$1:-}"
    if [ -z "$value" ] && [ -f "$env_file" ]; then
        value=$(sed -n "s/^$1=//p" "$env_file" | tail -n 1)
        value=${value#\"} value=${value%\"}
    fi
    if [ -z "$value" ]; then
        echo "$1 is not set (environment or .env.local)" >&2
        exit 2
    fi
    printf '%s' "$value"
}
local_dir=${JUNTO_CI_LOCAL_DIR:-$HOME/.cache/junto-ci}

# Run and job names go into shell commands on the runner: letters, digits and "-" only.
checked() {
    case "$1" in '' | *[!A-Za-z0-9-]*) echo "not a run or job name: $1" >&2; exit 2 ;; esac
}
host="" dir=""
linux_settings() {
    if [ -z "$host" ]; then
        host=$(setting JUNTO_CI_HOST)
        dir=$(setting JUNTO_CI_DIR)
    fi
}
# remote <command>: runs a shell command on the Linux host.
remote() { linux_settings; ssh -o BatchMode=yes "$host" "$1"; }
# on <runner> <command>: runs a shell command in the CI directory of the runner.
on() {
    if [ "$1" = linux ]; then
        linux_settings
        remote "cd '$dir' && $2"
    else
        (mkdir -p "$local_dir" && cd "$local_dir" && sh -c "$2")
    fi
}
commit_of() { git -C "$root" rev-parse --verify --quiet "$1^{commit}"; }

start() {
    runner=$1 commit=$(commit_of "$2") || { echo "no commit $2" >&2; exit 2; }
    shift 2
    if [ "$#" -eq 0 ]; then
        # shellcheck disable=SC2086 # the lists are words
        if [ "$runner" = linux ]; then set -- $linux_jobs; else set -- $macos_jobs; fi
    fi
    for job in "$@"; do checked "$job"; done
    run=$(date -u +%Y%m%dT%H%M%SZ)-$(printf '%.12s' "$commit")-$runner
    if [ "$runner" = linux ]; then
        linux_settings
        remote "test -d '$dir/repo.git' || git init --quiet --bare '$dir/repo.git'"
        # A push and the removal of unnamed objects by "forget" take the same lock.
        git -C "$root" push --quiet --receive-pack="flock '$dir/repo.lock' git-receive-pack" \
            "$host:$dir/repo.git" "$commit:refs/ci/$run"
        remote "mkdir -p '$dir/runs/$run' && cd '$dir/runs/$run' &&
            git --git-dir='$dir/repo.git' show '$commit:tools/ci/run-jobs.sh' >run-jobs.sh &&
            { setsid nohup sh run-jobs.sh '$dir' '$run' linux '$dir/repo.git' $* </dev/null >runner.log 2>&1 & }"
        echo "$run"
    else
        git -C "$root" update-ref "refs/ci/$run" "$commit"
        mkdir -p "$local_dir/runs/$run"
        git -C "$root" show "$commit:tools/ci/run-jobs.sh" >"$local_dir/runs/$run/run-jobs.sh"
        echo "$run"
        status=0
        sh "$local_dir/runs/$run/run-jobs.sh" "$local_dir" "$run" macos "$root" "$@" \
            >"$local_dir/runs/$run/runner.log" 2>&1 || status=$?
        (show "$run") || true
        return "$status"
    fi
}

# show <run>: prints the jobs of a run; returns 0 when every job passed, 1 when one failed,
# 3 while the run has not finished.
show() {
    run=$1
    case "$run" in *-linux) runner=linux ;; *-macos) runner=macos ;; *) echo "no run $run" >&2; return 2 ;; esac
    files=$(on "$runner" "cd 'runs/$run' 2>/dev/null || exit 0
        printf 'commit\t%s\n' \"\$(cat commit 2>/dev/null)\"
        printf 'jobs\t%s\n' \"\$(cat jobs 2>/dev/null)\"
        if [ -f current ]; then printf 'current\t%s\n' \"\$(cat current)\"; fi
        if [ -f finished ]; then printf 'finished\t%s\n' \"\$(cat finished)\"; fi
        if [ -f record.tsv ]; then sed 's/^/record\t/' record.tsv; fi")
    if [ -z "$files" ]; then echo "$run: no such run" >&2; return 2; fi
    commit=$(printf '%s\n' "$files" | sed -n 's/^commit\t//p')
    jobs=$(printf '%s\n' "$files" | sed -n 's/^jobs\t//p')
    current=$(printf '%s\n' "$files" | sed -n 's/^current\t//p')
    finished=$(printf '%s\n' "$files" | sed -n 's/^finished\t//p')
    echo "run $run  commit $commit"
    result=0
    for job in $jobs; do
        line=$(printf '%s\n' "$files" | awk -F '\t' -v job="$job" '$1 == "record" && $3 == job' | tail -n 1)
        if [ -n "$line" ]; then
            status=$(printf '%s' "$line" | cut -f 6)
            seconds=$(printf '%s' "$line" | cut -f 8)
            time="$((seconds / 60))m$((seconds % 60))s"
            if [ "$status" = 0 ]; then
                printf '  %-22s passed   %s\n' "$job" "$time"
            else
                result=1
                logs="$root/target/ci-logs/$run"
                mkdir -p "$logs"
                on "$runner" "cat 'runs/$run/logs/$job.log'" >"$logs/$job.log" || true
                printf '  %-22s FAILED   %s  exit %s  log %s\n' "$job" "$time" "$status" "$logs/$job.log"
            fi
        elif [ "$(printf '%s' "$current" | cut -f 1)" = "$job" ] && [ -z "$finished" ]; then
            printf '  %-22s running  since %s\n' "$job" "$(printf '%s' "$current" | cut -f 2)"
            [ "$result" -ne 0 ] || result=3
        elif [ -n "$finished" ]; then
            printf '  %-22s not run\n' "$job"
            result=1
        else
            printf '  %-22s waiting\n' "$job"
            [ "$result" -ne 0 ] || result=3
        fi
    done
    if [ -z "$finished" ]; then
        [ "$result" -ne 0 ] || result=3
    elif [ "$finished" != 0 ] && [ "$result" = 0 ]; then
        echo "  the runner ended with status $finished; see runs/$run/runner.log on the runner"
        result=1
    fi
    return "$result"
}

# The latest run of a commit on a runner, by the date at the start of the name.
latest() {
    on "$1" "ls runs 2>/dev/null | grep -- '-$(printf '%.12s' "$2")-$1\$' | sort | tail -n 1" || true
}

status() {
    case "$1" in
        *-linux | *-macos) (show "$1"); return ;;
    esac
    commit=$(commit_of "$1") || { echo "no commit or run $1" >&2; exit 2; }
    verdict=passed
    for runner in linux macos; do
        run=$(latest "$runner" "$commit")
        if [ -z "$run" ]; then
            echo "$runner: no run of $commit"
            verdict=incomplete
            continue
        fi
        result=0
        (show "$run") || result=$?
        if [ "$result" -eq 1 ]; then verdict=failed; fi
        if [ "$result" -ne 0 ] && [ "$verdict" = passed ]; then verdict=incomplete; fi
        # A run of only some jobs does not count for the commit.
        if [ "$runner" = linux ]; then wanted=$linux_jobs; else wanted=$macos_jobs; fi
        ran=$(on "$runner" "cat 'runs/$run/jobs'" || true)
        for job in $wanted; do
            case " $ran " in *" $job "*) ;; *) echo "  $job is not part of this run"; [ "$verdict" = failed ] || verdict=incomplete ;; esac
        done
    done
    echo "commit $commit: $verdict"
    [ "$verdict" = passed ]
}

forget() {
    run=$1
    case "$run" in
        *-linux)
            linux_settings
            remote "cd '$dir' && test -f 'runs/$run/finished' || { echo 'the run has not finished' >&2; exit 1; }
                rm -rf 'runs/$run' && git --git-dir=repo.git update-ref -d 'refs/ci/$run' &&
                flock repo.lock sh -c 'git --git-dir=repo.git reflog expire --expire=now --all &&
                    git --git-dir=repo.git gc --quiet --prune=now'" ;;
        *-macos)
            test -f "$local_dir/runs/$run/finished" || { echo 'the run has not finished' >&2; exit 1; }
            rm -rf "${local_dir:?}/runs/$run"
            git -C "$root" update-ref -d "refs/ci/$run" ;;
        *) echo "no run $run" >&2; exit 2 ;;
    esac
}

case "${1:-}" in
    run) shift; [ "$#" -ge 1 ] || set -- HEAD; start linux "$@" ;;
    local) shift; [ "$#" -ge 1 ] || set -- HEAD; start macos "$@" ;;
    status) [ "$#" -eq 2 ] || { echo "usage: $0 status <commit or run>" >&2; exit 2; }; checked "$2"; status "$2" ;;
    forget) [ "$#" -eq 2 ] || { echo "usage: $0 forget <run>" >&2; exit 2; }; checked "$2"; forget "$2" ;;
    *) sed -n '2,20p' "$0" >&2; exit 2 ;;
esac
