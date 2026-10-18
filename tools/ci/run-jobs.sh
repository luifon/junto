#!/bin/sh
# Runs CI jobs on one commit on this machine and records each job (ADR 0905). tools/ci/ci.sh
# starts it from the commit under test, on the Linux host, on the macOS machine or on the
# Windows machine:
#
#   run-jobs.sh <directory> <run> <runner> <repository> <job>...
#
# <directory> is the CI directory of this machine, <run> the name of the run, <runner>
# "linux", "macos" or "windows", <repository> a Git repository or a Git bundle file with the
# ref refs/ci/<run>. On Windows <directory> is a path such as C:/junto-ci, and the script runs
# in the Bash of Git for Windows. The run lives in <directory>/runs/<run>:
#
#   commit, jobs      the commit and the jobs of the run, written first
#   current           the job that runs and its start, while it runs
#   record.tsv        one line per finished job: commit, job, start, end (UTC), exit
#                     status, log file, seconds
#   logs/<job>.log    the output of the job
#   finished          the exit status of the run (0: every job passed), written last
#
# One run at a time: a run waits for the lock of tools/serial.sh, which on the Linux host and
# on the Windows machine is <directory>/run.lock. Jobs run one after the other, so that the limits of one job are the
# limits of the run. Before the first job and after each job, every container, network and
# volume whose name starts with junto-ci- or junto-ci_ is removed, except the cache volumes
# (Windows has no Docker); at the end the clone and a bundle are removed, also after a
# failure. Before the first job and after the last, tools/ci/caches.sh keeps the caches within
# their size caps and the free space of their disk above its level; when it cannot, the run
# starts no job.
set -eu
if [ "$#" -lt 5 ]; then
    echo "usage: $0 <directory> <run> <runner> <repository> <job>..." >&2
    exit 2
fi
directory=$1 run=$2 runner=$3 repository=$4
shift 4
here=$directory/runs/$run
checkout=$here/checkout

if [ -z "${JUNTO_CI_LOCKED:-}" ]; then
    # Until the locked part runs, a failure or a signal ends the run here: the clone is
    # removed and "finished" names the status.
    locked=""
    outer_end() {
        status=$?
        # The lock program runs the locked part as its child; that part ends its job and
        # cleans up on its own signal.
        if [ -n "$locked" ]; then
            pkill -TERM -P "$locked" 2>/dev/null || true
            kill "$locked" 2>/dev/null || true
            wait "$locked" 2>/dev/null || true
        fi
        rm -rf "$checkout" "$here/lock"
        if [ -f "$repository" ]; then rm -f "$repository"; fi
        echo "$status" >"$here/finished"
    }
    trap outer_end EXIT
    trap 'exit 143' INT TERM HUP
    mkdir -p "$here/logs"
    echo "$*" >"$here/jobs"
    # The clone holds the objects of the commit under test, in the ref refs/ci/<run>. Its files
    # are written by the locked part: Cargo decides from modification times whether a unit of
    # the shared target directory is up to date, and files written before an earlier run builds
    # there would be older than that run's output, which Cargo would then keep, although it was
    # compiled from the files of another commit.
    rm -rf "$checkout" "$here/lock"
    git init --quiet "$checkout"
    # The files as the commit holds them: Git for Windows sets core.autocrlf in its system
    # settings, which would give the scripts and the compared files CR LF line ends.
    git -C "$checkout" config core.autocrlf false
    git -C "$checkout" config core.longpaths true
    if [ -f "$repository" ]; then
        # A bundle: the fetch copies every object into the checkout.
        git -C "$checkout" fetch --quiet "$repository" "refs/ci/$run:refs/ci/$run"
    else
        objects=$(git -C "$repository" rev-parse --path-format=absolute --git-common-dir)/objects
        echo "$objects" >"$checkout/.git/objects/info/alternates"
        git -C "$checkout" fetch --quiet "$repository" "refs/ci/$run:refs/ci/$run"
        # Container commands cannot reach the host's object directory.
        git -C "$checkout" -c pack.threads=1 repack -a -d --quiet
        rm "$checkout/.git/objects/info/alternates"
    fi
    git -C "$checkout" rev-parse "refs/ci/$run" >"$here/commit"
    # The lock program and this script of the commit under test, which take the lock and run
    # the locked part.
    lock_files="tools/serial.sh tools/ci/run-jobs.sh"
    if [ "$runner" = windows ]; then lock_files="$lock_files tools/windows/lock.py"; fi
    for file in $lock_files; do
        mkdir -p "$here/lock/$(dirname -- "$file")"
        git -C "$checkout" show "refs/ci/$run:$file" >"$here/lock/$file"
    done
    if [ "$runner" != macos ]; then export JUNTO_HEAVY_LOCK="$directory/run.lock"; fi
    # tools/serial.sh takes the lock with Python on Windows (tools/windows/lock.py): the
    # Python of the setup, not a program of the same name in the PATH of the account.
    if [ "$runner" = windows ]; then PATH="$(cygpath -u "$directory")/tools/python:$PATH"; fi
    # The locked part writes "finished" itself; this part keeps its trap for the time the
    # run waits for the lock.
    # It runs in the background, so that a signal to this part ends it as well.
    JUNTO_CI_LOCKED=1 sh "$here/lock/tools/serial.sh" \
        sh "$here/lock/tools/ci/run-jobs.sh" "$directory" "$run" "$runner" "$repository" "$@" &
    locked=$!
    status=0
    wait "$locked" || status=$?
    locked=""
    trap - EXIT
    rm -rf "$checkout" "$here/lock"
    if [ -f "$repository" ]; then rm -f "$repository"; fi
    if [ ! -f "$here/finished" ]; then echo "$status" >"$here/finished"; fi
    exit "$status"
fi

pid=""
caches="junto-ci-cargo junto-ci-target junto-ci-uv junto-ci-npm"
sweep() {
    if [ "$runner" = windows ]; then return 0; fi
    for container in $(docker ps -aq --filter name=^junto-ci[-_]); do
        docker rm -f -v "$container" >/dev/null 2>&1 || true
    done
    for network in $(docker network ls -q --filter name=^junto-ci[-_]); do
        docker network rm "$network" >/dev/null 2>&1 || true
    done
    for volume in $(docker volume ls -q --filter name=^junto-ci[-_]); do
        case " $caches " in *" $volume "*) ;; *) docker volume rm "$volume" >/dev/null 2>&1 || true ;; esac
    done
    # The images that the deployment job builds on the macOS machine. On the Linux host the
    # Linux build image stays: it is the cache of its own build.
    if [ "$runner" = macos ]; then
        for image in $(docker images -q --filter reference='junto-ci-*'); do
            docker image rm -f "$image" >/dev/null 2>&1 || true
        done
    fi
}
finish() {
    status=$?
    if [ -n "$pid" ]; then kill "$pid" 2>/dev/null || true; fi
    sweep
    sh "$checkout/tools/ci/caches.sh" "$directory" "$runner" after >>"$here/caches.log" || true
    rm -f "$here/current"
    echo "$status" >"$here/finished"
}
trap finish EXIT
trap 'exit 143' INT TERM HUP
# Commands inside the jobs that take tools/serial.sh take a lock of this run.
export JUNTO_HEAVY_LOCK="$here/heavy.lock"
commit=$(cat "$here/commit")
git -C "$checkout" checkout --quiet --detach "refs/ci/$run"
cd "$checkout"
. tools/ci/pins.sh

# The programs that the Linux host lacks come from images: the uv program and the Docker
# command-line program, kept in <directory>/tools/bin for the image names of pins.sh. The
# file "images" is written only when both programs are there.
linux_tools() {
    tools=$directory/tools
    wanted="$JUNTO_CI_UV_IMAGE $JUNTO_CI_DOCKER_CLI_IMAGE"
    if [ "$(cat "$tools/images" 2>/dev/null || true)" = "$wanted" ]; then return 0; fi
    rm -rf "$tools" && mkdir -p "$tools/bin" \
        && docker create --name junto-ci-tools-uv "$JUNTO_CI_UV_IMAGE" /uv >/dev/null \
        && docker cp junto-ci-tools-uv:/uv "$tools/bin/uv" \
        && docker rm junto-ci-tools-uv >/dev/null \
        && docker create --name junto-ci-tools-docker "$JUNTO_CI_DOCKER_CLI_IMAGE" >/dev/null \
        && docker cp junto-ci-tools-docker:/usr/local/bin/docker "$tools/bin/docker" \
        && docker rm junto-ci-tools-docker >/dev/null \
        && [ -x "$tools/bin/uv" ] && [ -x "$tools/bin/docker" ] \
        && echo "$wanted" >"$tools/images" || return 1
}

# The limits of one job on the Linux host: at most 8 processors and 12 GB (11 GiB) of memory
# over all of its containers, without swap. tools/linux/run.sh gives its build container the
# processors and memory below, and PostgreSQL, LiveKit and the control container 4
# processors and 2.4 GiB together. A host with fewer processors gives fewer.
host_cpus=8
if [ "$runner" = linux ]; then
    host_cpus=$(docker info --format '{{.NCPU}}')
    if [ "$host_cpus" -gt 8 ]; then host_cpus=8; fi
fi
build_cpus=$((host_cpus > 2 ? host_cpus - 2 : 1))
rust_cpus=$((host_cpus > 4 ? host_cpus - 4 : 1))
postgres_cpus=2.75
if [ "$host_cpus" -lt 5 ]; then postgres_cpus=0.75; fi
linux_container() {
    JUNTO_LINUX_PREFIX=junto-ci JUNTO_LINUX_BUILDER=limited JUNTO_LINUX_CPUS=$rust_cpus \
        JUNTO_LINUX_POSTGRES_CPUS=$postgres_cpus JUNTO_LINUX_MEMORY=8g tools/linux-container.sh "$@"
}
# The Windows job: the tools that tools/windows/ci-setup.ps1 installed below the CI directory,
# the programs of Git for Windows and of Windows, and no other program of the machine. The
# PostgreSQL download, the Cargo registry and the target directory stay in the CI directory
# from one run to the next; the temporary directory is emptied before and after the job.
rust_windows() {
    tools=$(cygpath -u "$directory")
    windows=$(cygpath -u -W)
    git=$(dirname -- "$(command -v git)")
    export CARGO_HOME="$directory/cargo" RUSTUP_HOME="$directory/rustup"
    export CARGO_TARGET_DIR="$directory/cargo-target" JUNTO_TEST_POSTGRES_DOWNLOADS="$directory/postgres"
    export TMP="$directory/tmp" TEMP="$directory/tmp" TMPDIR="$directory/tmp"
    export PATH="$tools/tools/python:$tools/cargo/bin:$git:/usr/bin:$windows/System32:$windows:$windows/System32/Wbem:$windows/System32/WindowsPowerShell/v1.0"
    rm -rf "$directory/tmp" && mkdir -p "$directory/tmp" || return 1
    version=$(sed -n 's/^channel = "\(.*\)"$/\1/p' rust-toolchain.toml)
    status=0
    # --no-self-update: rustup stays at the version of the setup (ADR 0905).
    rustup toolchain install "$version" --no-self-update --profile minimal --component rustfmt --component clippy \
        && tools/ci/rust-other-systems.sh || status=$?
    rm -rf "$directory/tmp"
    return "$status"
}
job() {
    case "$runner:$1" in
        linux:rust-linux)
            JUNTO_LINUX_PREFIX=junto-ci JUNTO_LINUX_BUILDER=limited JUNTO_LINUX_CPUS=$rust_cpus \
                JUNTO_LINUX_POSTGRES_CPUS=$postgres_cpus JUNTO_LINUX_MEMORY=8g tools/ci/rust-linux.sh ;;
        linux:licences)
            JUNTO_CI_TOOLS=/target/ci-tools linux_container tools/ci/licences.sh ;;
        linux:transcription-worker)
            linux_tools || return 1
            docker run --rm --name junto-ci-worker --cpus "$host_cpus" --memory 11g --memory-swap 11g \
                --mount "type=bind,src=$checkout,dst=/src" -w /src \
                --mount "type=bind,src=$directory/tools/bin,dst=/opt/junto-ci/bin,readonly" \
                --mount type=volume,src=junto-ci-uv,dst=/cache \
                -e UV_CACHE_DIR=/cache/uv -e UV_PYTHON_INSTALL_DIR=/cache/python -e UV_LINK_MODE=copy \
                -e PATH=/opt/junto-ci/bin:/usr/local/bin:/usr/bin:/bin \
                "$JUNTO_CI_PYTHON_IMAGE" tools/linux/run-as-user.sh 1000 1000 /src /cache -- \
                    tools/ci/transcription-worker.sh ;;
        linux:layout-page)
            # The page's tests start LiveKit with tools/with-test-livekit.sh, which publishes
            # its ports on 127.0.0.1 of the Docker host; this container shares the host's
            # network to reach them. Together with LiveKit: 8 processors and 11 GiB.
            linux_tools || return 1
            docker run --rm --name junto-ci-layout --network host --cpus "$build_cpus" \
                --memory 9g --memory-swap 9g --shm-size 1g \
                --mount "type=bind,src=$checkout,dst=/src" -w /src \
                --mount "type=bind,src=$directory/tools/bin,dst=/opt/junto-ci/bin,readonly" \
                --mount type=bind,src=/var/run/docker.sock,dst=/var/run/docker.sock \
                --mount type=volume,src=junto-ci-npm,dst=/cache \
                -e JUNTO_CI_LAYOUT_DEPS_READY=1 -e npm_config_cache=/cache/npm -e PLAYWRIGHT_BROWSERS_PATH=/cache/playwright \
                -e PATH=/opt/junto-ci/bin:/usr/local/bin:/usr/bin:/bin \
                -e JUNTO_TEST_LIVEKIT_IMAGE -e JUNTO_TEST_LIVEKIT_CONTAINER=junto-ci-layout-livekit \
                -e JUNTO_TEST_LIVEKIT_PORT=37880 -e JUNTO_TEST_LIVEKIT_TCP_PORT=37881 \
                -e JUNTO_TEST_LIVEKIT_UDP_PORT=37882 -e JUNTO_TEST_WEBHOOK_ADDRESS=127.0.0.1:37890 \
                -e JUNTO_TEST_LIVEKIT_CPUS=$((host_cpus - build_cpus)) -e JUNTO_TEST_LIVEKIT_MEMORY=2g \
                "$JUNTO_CI_NODE_IMAGE" sh -c 'cd recording-layout && npm ci --ignore-scripts &&
                    npx --no-install playwright install-deps chromium && cd /src &&
                    exec tools/linux/run-as-user.sh 1000 1000 /src /cache -- tools/ci/layout-page.sh'  ;;
        macos:rust-macos)
            CARGO_TARGET_DIR="$directory/cargo-target" tools/ci/rust-other-systems.sh ;;
        macos:deployment)
            tools/ci/deployment.sh ;;
        windows:rust-windows)
            rust_windows ;;
        *)
            echo "no job $1 on the $runner runner" >&2
            return 2 ;;
    esac
}

now() { date -u +%Y-%m-%dT%H:%M:%SZ; }
# Containers and networks of an earlier run that ended without its cleanup.
sweep
sh "$checkout/tools/ci/caches.sh" "$directory" "$runner" before >>"$here/caches.log"
failed=0
for name in "$@"; do
    start=$(now)
    began=$(date +%s)
    printf '%s\t%s\n' "$name" "$start" >"$here/current"
    # The job runs in the background, so that a signal to this script ends it at once. It
    # does not see the terminal of the runner.
    job "$name" </dev/null >"$here/logs/$name.log" 2>&1 &
    pid=$!
    status=0
    wait "$pid" || status=$?
    pid=""
    sweep
    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$commit" "$name" "$start" "$(now)" "$status" \
        "logs/$name.log" "$(($(date +%s) - began))" >>"$here/record.tsv"
    if [ "$status" -ne 0 ]; then failed=1; fi
done
exit "$failed"
