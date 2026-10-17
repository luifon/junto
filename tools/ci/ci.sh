#!/bin/sh
# Runs the CI jobs of a commit on a Linux host and on a Windows machine over SSH and on this
# macOS machine, and prints their results (ADR 0905).
#
#   ci.sh run <commit> [job...]     sends the commit to the Linux host and to the Windows
#                                   machine and starts their jobs there, detached: the runs
#                                   continue when this machine sleeps or the connection
#                                   drops. Prints the run names. With jobs, only the runners
#                                   that have them.
#   ci.sh local <commit> [job...]   runs the macOS jobs here, through tools/serial.sh.
#   ci.sh status <commit or run>    prints each job of a run: passed, failed, running or
#                                   waiting, with its time, and copies the log of a failed
#                                   job to target/ci-logs/<run>/. For a commit: its latest
#                                   run on each runner, and whether the commit passed.
#   ci.sh forget <run>              removes a run, its ref and, when no other ref keeps it,
#                                   its commit from the runner that has it.
#   ci.sh setup-windows             installs the pinned tools of the Windows job on the
#                                   Windows machine (tools/windows/ci-setup.ps1 of this
#                                   working tree). Exit 3: Windows must restart first.
#
# The SSH alias of the Linux host and its CI directory come from JUNTO_CI_HOST and
# JUNTO_CI_DIR, in the environment or in .env.local of the repository. The Windows machine:
# JUNTO_CI_WINDOWS_HOST (address or SSH alias), JUNTO_CI_WINDOWS_DIR (a folder such as
# C:/junto-ci), and optionally JUNTO_CI_WINDOWS_USER and JUNTO_CI_WINDOWS_KEY (a private key
# file). The CI directory of this machine is JUNTO_CI_LOCAL_DIR (default ~/.cache/junto-ci).
set -eu
root=$(CDPATH='' cd -- "$(dirname -- "$0")/../.." && pwd)

# The jobs of each runner. A commit has passed CI when its latest run on each runner has
# finished with every job of this list passed.
linux_jobs="rust-linux licences transcription-worker layout-page"
macos_jobs="rust-macos deployment"
windows_jobs="rust-windows"
runners="linux macos windows"
jobs_of() {
    case "$1" in linux) echo "$linux_jobs" ;; macos) echo "$macos_jobs" ;; windows) echo "$windows_jobs" ;; esac
}

# .env.local of this working tree, or else of the main working tree of the repository.
env_file=$root/.env.local
if [ ! -f "$env_file" ]; then
    env_file=$(git -C "$root" rev-parse --path-format=absolute --git-common-dir)/../.env.local
fi
# optional <name>: the value of a setting, empty when it is not set.
optional() {
    eval "value=\${$1:-}"
    if [ -z "$value" ] && [ -f "$env_file" ]; then
        value=$(sed -n "s/^$1=//p" "$env_file" | tail -n 1)
        value=${value#\"} value=${value%\"}
    fi
    printf '%s' "$value"
}
setting() {
    value=$(optional "$1")
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

# The Windows machine. Its SSH server starts a command in the shell of the account (Windows
# PowerShell or cmd.exe). Shell commands go to the Bash of Git for Windows that the setup
# installed, which reads them from its standard input, so that no command text passes through
# the quoting rules of the account's shell.
windows_host="" windows_dir="" windows_user="" windows_key=""
windows_settings() {
    if [ -z "$windows_host" ]; then
        windows_host=$(setting JUNTO_CI_WINDOWS_HOST)
        windows_dir=$(setting JUNTO_CI_WINDOWS_DIR)
        # The folder goes unquoted into commands of the account's shell and of PowerShell.
        case "$windows_dir" in
            [A-Za-z]:/*) ;;
            *) echo "JUNTO_CI_WINDOWS_DIR is not a folder such as C:/junto-ci" >&2; exit 2 ;;
        esac
        case "$windows_dir" in
            *[!A-Za-z0-9:/_.-]*) echo "JUNTO_CI_WINDOWS_DIR has a character other than letters, digits and :/_.-" >&2; exit 2 ;;
        esac
        windows_user=$(optional JUNTO_CI_WINDOWS_USER)
        windows_key=$(optional JUNTO_CI_WINDOWS_KEY)
        case "$windows_key" in \~/*) windows_key=$HOME/${windows_key#\~/} ;; esac
        case "$windows_user" in *\"*) echo "JUNTO_CI_WINDOWS_USER has a quote" >&2; exit 2 ;; esac
    fi
}
# windows_ssh <program> <argument>...: runs ssh or scp with the options of the Windows
# machine. The user name can hold spaces; ssh reads a quoted value of an option.
windows_ssh() {
    windows_settings
    program=$1
    shift
    if [ -n "$windows_user" ]; then set -- -o "User=\"$windows_user\"" "$@"; fi
    if [ -n "$windows_key" ]; then set -- -o IdentitiesOnly=yes -i "$windows_key" "$@"; fi
    "$program" -o BatchMode=yes "$@"
}
# windows_bash <script>: runs a script in the Bash of the Windows machine.
windows_bash() {
    windows_settings
    printf '%s\n' "$1" | windows_ssh ssh "$windows_host" "$windows_dir/tools/git/bin/bash.exe -s"
}
# on <runner> <command>: runs a shell command in the CI directory of the runner.
on() {
    case "$1" in
        linux)
            linux_settings
            remote "cd '$dir' && $2" ;;
        windows)
            windows_settings
            windows_bash "cd '$windows_dir' && $2" ;;
        *)
            (mkdir -p "$local_dir" && cd "$local_dir" && sh -c "$2") ;;
    esac
}
commit_of() { git -C "$root" rev-parse --verify --quiet "$1^{commit}"; }

start() {
    runner=$1 commit=$(commit_of "$2") || { echo "no commit $2" >&2; exit 2; }
    shift 2
    if [ "$#" -eq 0 ]; then
        # shellcheck disable=SC2046 # the lists are words
        set -- $(jobs_of "$runner")
    fi
    for job in "$@"; do checked "$job"; done
    run=$(date -u +%Y%m%dT%H%M%SZ)-$(printf '%.12s' "$commit")-$runner
    if [ "$runner" = windows ]; then
        windows_settings
        here=$windows_dir/runs/$run
        # The commit goes to the machine as a bundle with the ref refs/ci/<run>; run-jobs.sh
        # fetches it into its clone and removes it. The machine keeps no repository.
        files=$(mktemp -d)
        status=0
        git -C "$root" update-ref "refs/ci/$run" "$commit"
        git -C "$root" bundle create --quiet "$files/commit.bundle" "refs/ci/$run" || status=$?
        git -C "$root" update-ref -d "refs/ci/$run"
        if [ "$status" -eq 0 ]; then
            # A commit from before the Windows runner has neither file.
            git -C "$root" show "$commit:tools/ci/run-jobs.sh" >"$files/run-jobs.sh" 2>/dev/null \
                && git -C "$root" show "$commit:tools/windows/start-run.ps1" >"$files/start-run.ps1" 2>/dev/null \
                || { status=2; echo "the commit $commit has no Windows runner (tools/windows/start-run.ps1)" >&2; }
        fi
        if [ "$status" -eq 0 ]; then
            # start-run.ps1 starts start.sh in the desktop session of the account and waits for
            # the file "started".
            printf '%s\n' "cd '$here' && : >started && exec sh run-jobs.sh '$windows_dir' '$run' windows '$here/commit.bundle' $* </dev/null >runner.log 2>&1" \
                >"$files/start.sh"
            windows_bash "mkdir -p '$here'" \
                && windows_ssh scp -q "$files/commit.bundle" "$files/run-jobs.sh" "$files/start-run.ps1" \
                    "$files/start.sh" "$windows_host:$here/" \
                && windows_ssh ssh "$windows_host" \
                    "powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File $here/start-run.ps1 -Directory $windows_dir -Run $run" \
                || status=$?
        fi
        rm -rf "$files"
        if [ "$status" -ne 0 ]; then echo "the run $run did not start on the Windows machine" >&2; exit "$status"; fi
        echo "$run"
    elif [ "$runner" = linux ]; then
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
    case "$run" in
        *-linux) runner=linux ;;
        *-macos) runner=macos ;;
        *-windows) runner=windows ;;
        *) echo "no run $run" >&2; return 2 ;;
    esac
    files=$(on "$runner" "cd 'runs/$run' 2>/dev/null || exit 0
        printf 'commit\t%s\n' \"\$(cat commit 2>/dev/null)\"
        printf 'jobs\t%s\n' \"\$(cat jobs 2>/dev/null)\"
        if [ -f current ]; then printf 'current\t%s\n' \"\$(cat current)\"; fi
        if [ -f finished ]; then printf 'finished\t%s\n' \"\$(cat finished)\"; fi
        if [ -f record.tsv ]; then sed 's/^/record\t/' record.tsv; fi
        $(ended_check "$runner" "$run")")
    if [ -z "$files" ]; then echo "$run: no such run" >&2; return 2; fi
    commit=$(printf '%s\n' "$files" | sed -n 's/^commit\t//p')
    jobs=$(printf '%s\n' "$files" | sed -n 's/^jobs\t//p')
    current=$(printf '%s\n' "$files" | sed -n 's/^current\t//p')
    finished=$(printf '%s\n' "$files" | sed -n 's/^finished\t//p')
    ended=$(printf '%s\n' "$files" | sed -n 's/^ended\t//p')
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
        elif [ "$(printf '%s' "$current" | cut -f 1)" = "$job" ] && [ -z "$finished" ] && [ -n "$ended" ]; then
            printf '  %-22s ENDED    without a result, started %s\n' "$job" "$(printf '%s' "$current" | cut -f 2)"
            result=1
        elif [ "$(printf '%s' "$current" | cut -f 1)" = "$job" ] && [ -z "$finished" ]; then
            printf '  %-22s running  since %s\n' "$job" "$(printf '%s' "$current" | cut -f 2)"
            [ "$result" -ne 0 ] || result=3
        elif [ -n "$finished" ] || [ -n "$ended" ]; then
            printf '  %-22s not run\n' "$job"
            result=1
        else
            printf '  %-22s waiting\n' "$job"
            [ "$result" -ne 0 ] || result=3
        fi
    done
    if [ -z "$finished" ] && [ -n "$ended" ]; then
        echo "  the processes of the run ended without its result (a logoff, a restart or a stop of Windows); ci.sh forget $run removes it"
        result=1
    elif [ -z "$finished" ]; then
        [ "$result" -ne 0 ] || result=3
    elif [ "$finished" != 0 ] && [ "$result" = 0 ]; then
        echo "  the runner ended with status $finished; see runs/$run/runner.log on the runner"
        result=1
    fi
    return "$result"
}

# ended_check <runner> <run>: for a Windows run without "finished", a shell command, run in the
# folder of the run, that prints "ended<tab>1" when the process that start-run.ps1 recorded for
# the run no longer runs. A logoff, a restart or a stop of Windows ends the processes of a run
# before they write "finished". Empty for the other runners, whose runs are started by a
# process that writes "finished" on every end of its own.
ended_check() {
    if [ "$1" = windows ]; then
        windows_settings
        printf '%s' "if [ ! -f finished ] && [ -f process ] && powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File start-run.ps1 -Directory '$windows_dir' -Run '$2' -Ended >/dev/null; then printf 'ended\t1\n'; fi"
    fi
}

# The latest run of a commit on a runner, by the date at the start of the name.
latest() {
    on "$1" "ls runs 2>/dev/null | grep -- '-$(printf '%.12s' "$2")-$1\$' | sort | tail -n 1" || true
}

status() {
    case "$1" in
        *-linux | *-macos | *-windows) (show "$1"); return ;;
    esac
    commit=$(commit_of "$1") || { echo "no commit or run $1" >&2; exit 2; }
    verdict=passed
    for runner in $runners; do
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
        wanted=$(jobs_of "$runner")
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
        *-windows)
            on windows "cd 'runs/$run' || exit 1
                test -f finished || { $(ended_check windows "$run"); } | grep -q . ||
                    { echo 'the run has not finished' >&2; exit 1; }
                cd ../.. && rm -rf 'runs/$run'" ;;
        *-macos)
            test -f "$local_dir/runs/$run/finished" || { echo 'the run has not finished' >&2; exit 1; }
            rm -rf "${local_dir:?}/runs/$run"
            git -C "$root" update-ref -d "refs/ci/$run" ;;
        *) echo "no run $run" >&2; exit 2 ;;
    esac
}

# run_remote <commit> [job...]: starts a run on each remote runner, or with jobs, on each
# remote runner that has one of them, with those of its jobs.
run_remote() {
    commit=$1
    shift
    started=""
    for runner in linux windows; do
        wanted=$(jobs_of "$runner") picked=""
        for job in "$@"; do
            case " $wanted " in *" $job "*) picked="$picked $job" ;; esac
        done
        if [ "$#" -gt 0 ] && [ -z "$picked" ]; then continue; fi
        # shellcheck disable=SC2086 # the picked jobs are words
        (start "$runner" "$commit" $picked)
        started="$started $runner"
    done
    if [ -z "$started" ]; then echo "no remote runner has the jobs $*" >&2; exit 2; fi
}

# setup_windows: copies tools/windows/ci-setup.ps1 of this working tree to the Windows machine
# and runs it there with Windows PowerShell.
setup_windows() {
    windows_settings
    windows_ssh ssh "$windows_host" \
        "powershell.exe -NoProfile -NonInteractive -Command New-Item -ItemType Directory -Force -Path $windows_dir | Out-Null"
    windows_ssh scp -q "$root/tools/windows/ci-setup.ps1" "$windows_host:$windows_dir/ci-setup.ps1"
    windows_ssh ssh "$windows_host" \
        "powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File $windows_dir/ci-setup.ps1 -Directory $windows_dir"
}

case "${1:-}" in
    run) shift; [ "$#" -ge 1 ] || set -- HEAD; run_remote "$@" ;;
    setup-windows) [ "$#" -eq 1 ] || { echo "usage: $0 setup-windows" >&2; exit 2; }; setup_windows ;;
    local) shift; [ "$#" -ge 1 ] || set -- HEAD; start macos "$@" ;;
    status) [ "$#" -eq 2 ] || { echo "usage: $0 status <commit or run>" >&2; exit 2; }; checked "$2"; status "$2" ;;
    forget) [ "$#" -eq 2 ] || { echo "usage: $0 forget <run>" >&2; exit 2; }; checked "$2"; forget "$2" ;;
    *) sed -n '2,25p' "$0" >&2; exit 2 ;;
esac
