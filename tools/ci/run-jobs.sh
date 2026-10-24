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
#   commit.git        a bare repository with the objects of the commit, while the run runs
#   commit, jobs      the commit and the jobs of the run, written first
#   current           the job that runs and its start, while it runs
#   record.tsv        one line per finished job: commit, job, start, end (UTC), exit
#                     status, log file, seconds
#   logs/<job>.log    the output of the job
#   finished          the exit status of the run (0: every job passed), written last
#
# One run at a time: a run waits for the lock of tools/serial.sh --inherited, which on the
# Linux host and on the Windows machine is <directory>/run.lock; the processes of the run hold
# the lock until the last of them has ended. Jobs run one after the other, so that the limits
# of one job are the limits of the run. Each job runs in a group of processes of its own
# (tools/ci/group.py), and the next step begins only after every process of the group has
# ended, also when the run is ended by a signal. Before the checkout is written, before the
# first job and after each job, every container, network and volume whose name starts with
# junto-ci- or junto-ci_ is removed, except the cache volumes (Windows has no Docker); at the
# end the repository of the run and a bundle are removed, also after a failure. Every run of
# the machine uses the one checkout <directory>/checkout. Before the first job and after
# the last, tools/ci/caches.py keeps the caches within their limits; when the free space is too
# low for the run, the run starts no job.
set -eu
if [ "$#" -lt 5 ]; then
    echo "usage: $0 <directory> <run> <runner> <repository> <job>..." >&2
    exit 2
fi
directory=$1 run=$2 runner=$3 repository=$4
shift 4
here=$directory/runs/$run
# One checkout at one path for every run of the machine. Cargo keeps the output of a unit whose
# source files are not newer than that output, and the compile-time paths of that output
# (CARGO_MANIFEST_DIR, file!) name the checkout that built it. With a checkout per run, kept
# output named the checkout of an earlier run, which no longer existed; with one path, Git
# rewrites only the files that differ from the earlier commit, those are newer than every
# earlier output, and the paths of kept output stay valid.
checkout=$directory/checkout
objects=$here/commit.git

if [ -z "${JUNTO_CI_LOCKED:-}" ]; then
    # Until the locked part runs, a failure or a signal ends the run here: the repository of the
    # run is removed and "finished" names the status. This part never changes the checkout,
    # which another run can be using.
    locked=""
    outer_end() {
        status=$?
        trap '' INT TERM HUP
        # The lock program sends the signal on to the locked part and ends after it, so the
        # repository of the run stays until that part has ended its job and checked the caches;
        # a run that waits for the lock ends at once. On Windows the signal ends the lock program, and its keeper
        # ends every process of the run before it releases the lock (tools/windows/lock.py).
        if [ -n "$locked" ]; then
            kill -TERM "$locked" 2>/dev/null || true
            wait "$locked" 2>/dev/null || true
        fi
        rm -rf "$objects" "$here/lock"
        if [ -f "$repository" ]; then rm -f "$repository"; fi
        if [ ! -f "$here/finished" ]; then echo "$status" >"$here/finished"; fi
    }
    trap outer_end EXIT
    # The first signal ignores the next ones before it ends the script: a Ctrl-C reaches this
    # part and the locked part, and the lock program sends it on once more; a second signal
    # during the end would run "exit" again inside the cleanup and end it.
    trap 'trap "" INT TERM HUP; exit 143' INT TERM HUP
    mkdir -p "$here/logs"
    echo "$*" >"$here/jobs"
    # The objects of the commit under test, in the ref refs/ci/<run> of a bare repository of the
    # run. The locked part fetches them into the checkout and writes the files there.
    rm -rf "$objects" "$here/lock"
    git init --quiet --bare "$objects"
    if [ -f "$repository" ]; then
        # A bundle: the fetch copies every object.
        git -C "$objects" fetch --quiet "$repository" "refs/ci/$run:refs/ci/$run"
    else
        alternate=$(git -C "$repository" rev-parse --path-format=absolute --git-common-dir)/objects
        echo "$alternate" >"$objects/objects/info/alternates"
        git -C "$objects" fetch --quiet "$repository" "refs/ci/$run:refs/ci/$run"
        # The run does not depend on the source repository after this point.
        git -C "$objects" -c pack.threads=1 repack -a -d --quiet
        rm "$objects/objects/info/alternates"
    fi
    git -C "$objects" rev-parse "refs/ci/$run" >"$here/commit"
    # The lock programs and this script of the commit under test, which take the lock and run
    # the locked part.
    for file in tools/serial.sh tools/windows/lock.py tools/ci/run-jobs.sh; do
        mkdir -p "$here/lock/$(dirname -- "$file")"
        git -C "$objects" show "refs/ci/$run:$file" >"$here/lock/$file"
    done
    # The run lock: on the Linux host and on the Windows machine a lock file of the CI directory;
    # on the macOS machine the lock of the heavy commands of that machine, which tools/serial.sh
    # names when JUNTO_HEAVY_LOCK is not set, or JUNTO_CI_RUN_LOCK (for the tests of this
    # script). A JUNTO_HEAVY_LOCK of the caller is not used: two runs with two locks would write
    # the one checkout at the same time.
    if [ "$runner" != macos ]; then
        export JUNTO_HEAVY_LOCK="$directory/run.lock"
    elif [ -n "${JUNTO_CI_RUN_LOCK:-}" ]; then
        export JUNTO_HEAVY_LOCK="$JUNTO_CI_RUN_LOCK"
    else
        unset JUNTO_HEAVY_LOCK
    fi
    # tools/serial.sh --inherited takes the lock with Python (tools/windows/lock.py); on
    # Windows the Python of the setup, not a program of the same name in the PATH of the
    # account.
    if [ "$runner" = windows ]; then PATH="$(cygpath -u "$directory")/tools/python:$PATH"; fi
    # The locked part writes "finished" itself; this part keeps its trap for the time the
    # run waits for the lock.
    # It runs in the background, so that a signal to this part ends it as well.
    JUNTO_CI_LOCKED=1 sh "$here/lock/tools/serial.sh" --inherited \
        sh "$here/lock/tools/ci/run-jobs.sh" "$directory" "$run" "$runner" "$repository" "$@" &
    locked=$!
    status=0
    wait "$locked" || status=$?
    locked=""
    trap - EXIT
    rm -rf "$objects" "$here/lock"
    if [ -f "$repository" ]; then rm -f "$repository"; fi
    if [ ! -f "$here/finished" ]; then echo "$status" >"$here/finished"; fi
    exit "$status"
fi

# The locked part. With JUNTO_CI_JOB set, the script runs that one job in the group of
# tools/ci/group.py: the same settings, without the traps and the checkout.
pid="" group=""
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
# end_job: ends every process of the running job and waits until each has ended. A signal from
# the Bash of Git for Windows ends a native program at once, so on Windows the job object is
# ended by its name.
end_job() {
    if [ "$runner" = windows ]; then
        python3 "$checkout/tools/ci/group.py" --end "$group" || true
    else
        kill -TERM "$pid" 2>/dev/null || true
    fi
    wait "$pid" 2>/dev/null || true
    pid=""
}
caches_check() {
    JUNTO_CI_PYTHON_IMAGE=${JUNTO_CI_PYTHON_IMAGE:-} \
        python3 "$checkout/tools/ci/caches.py" "$directory" "$runner" "$1" "$run" >>"$here/caches.log"
}
finish() {
    status=$?
    # A second signal, such as the INT of a terminal that the lock program also sends on, must
    # not end the cleanup.
    trap '' INT TERM HUP
    if [ -n "$pid" ]; then end_job; fi
    sweep
    caches_check after || true
    rm -f "$here/current"
    echo "$status" >"$here/finished"
}
# prepare_checkout: the checkout at the commit of this run. Its status is that of the first step
# that fails: the steps are joined with &&, because a function in the condition of "if" runs
# without set -e.
prepare_checkout() {
    if [ -d "$checkout/.git" ]; then
        find "$checkout/.git" -name '*.lock' -type f -exec rm -f {} + || return
    else
        { rm -rf "$checkout" && git init --quiet "$checkout"; } || return
    fi
    # The files as the commit holds them: Git for Windows sets core.autocrlf in its system
    # settings, which would give the scripts and the compared files CR LF line ends.
    # The commit comes with its history, for the upgrade test of the deployment job. The checkout
    # rewrites the files that differ; clean removes every file that the commit does not hold,
    # also those that an earlier run wrote. Container commands read only this repository.
    git -C "$checkout" config core.autocrlf false \
        && git -C "$checkout" config core.longpaths true \
        && git -C "$checkout" fetch --quiet --no-tags "$objects" "+refs/ci/$run:refs/ci/$run" \
        && git -C "$checkout" checkout --quiet --force --detach "refs/ci/$run" \
        && git -C "$checkout" clean -ffdxq \
        && git -C "$checkout" update-ref -d "refs/ci/$run" \
        && git -C "$checkout" gc --auto --quiet
}
# checked_checkout: the checkout holds exactly the commit of the run: its HEAD is that commit, and
# Git sees no changed, added, removed or ignored file. Git compares the content of a file only when
# its recorded time is not older than the index, and otherwise trusts the time that the checkout
# recorded; read-tree gives an index without recorded times, so status compares the content of
# every file.
checked_checkout() {
    head=$(git -C "$checkout" rev-parse HEAD 2>/dev/null) || head=""
    changes=$(git -C "$checkout" read-tree HEAD 2>&1 && git -C "$checkout" status --porcelain --ignored 2>&1) \
        || changes="git status failed: $changes"
    if [ "$head" != "$(cat "$here/commit")" ]; then
        echo "the checkout holds ${head:-no commit}, not the commit $(cat "$here/commit") of the run" >&2
        return 1
    fi
    if [ -n "$changes" ]; then
        printf 'the checkout differs from the commit of the run:\n%s\n' "$changes" | head -n 20 >&2
        return 1
    fi
}
if [ -z "${JUNTO_CI_JOB:-}" ]; then
    trap finish EXIT
    trap 'trap "" INT TERM HUP; exit 143' INT TERM HUP
    # Containers of an earlier run that ended without its cleanup can still write into a cache
    # volume; they are removed before the files of the checkout are written.
    sweep
    # A run that was ended while Git wrote the checkout (a stop of Windows, a KILL) leaves the
    # lock files of Git. No other Git process uses the checkout while this run holds the run
    # lock, so they are removed; when Git still fails, the checkout is created again.
    if ! prepare_checkout; then
        echo "the checkout could not be updated; it is created again" >&2
        rm -rf "$checkout"
        if ! prepare_checkout; then
            echo "the checkout could not be created" >&2
            exit 1
        fi
    fi
    # The jobs test these files, and the run records their result for this commit.
    if ! checked_checkout; then
        exit 1
    fi
    # Commands inside the jobs that take tools/serial.sh take a lock of this run.
    export JUNTO_HEAVY_LOCK="$here/heavy.lock"
fi
commit=$(cat "$here/commit")
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
# processors and 2.4 GiB together. CI reserves the upper eight processors on a 16-CPU
# host; the worker locks use CPUs 0-7. A smaller host must serialize workers with CI.
host_cpus=8
cpu_first=0
if [ "$runner" = linux ]; then
    total_cpus=$(docker info --format '{{.NCPU}}')
    host_cpus=$total_cpus
    if [ "$host_cpus" -gt 8 ]; then host_cpus=8; fi
    cpu_first=$((total_cpus - host_cpus))
fi
ci_set="$cpu_first-$((cpu_first + host_cpus - 1))"
build_cpus=$((host_cpus > 2 ? host_cpus - 2 : 1))
rust_cpus=$((host_cpus > 4 ? host_cpus - 4 : 1))
postgres_cpus=2.75
if [ "$host_cpus" -lt 5 ]; then postgres_cpus=0.75; fi
if [ "$runner" = linux ]; then
    export JUNTO_LINUX_CPU_FIRST=$cpu_first
    service_first=$((cpu_first + rust_cpus))
    if [ "$((service_first + rust_cpus))" -gt "$total_cpus" ]; then service_first=$cpu_first; fi
    export JUNTO_LINUX_SERVICE_CPU_FIRST=$service_first
fi
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
        linux:rust-linux-devices)
            JUNTO_LINUX_PREFIX=junto-ci JUNTO_LINUX_BUILDER=limited JUNTO_LINUX_CPUS=$rust_cpus \
                JUNTO_LINUX_POSTGRES_CPUS=$postgres_cpus JUNTO_LINUX_MEMORY=8g tools/ci/rust-linux-devices.sh ;;
        linux:licences)
            JUNTO_CI_TOOLS=/target/ci-tools linux_container tools/ci/licences.sh ;;
        linux:transcription-worker)
            linux_tools || return 1
            docker run --rm --name junto-ci-worker --cpuset-cpus "$ci_set" --cpus "$host_cpus" --memory 11g --memory-swap 11g \
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
            docker run --rm --name junto-ci-layout --network host --cpuset-cpus "$ci_set" --cpus "$build_cpus" \
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
                -e JUNTO_TEST_LIVEKIT_CPUSET="$ci_set" \
                -e JUNTO_TEST_LIVEKIT_CPUS=$((host_cpus - build_cpus)) -e JUNTO_TEST_LIVEKIT_MEMORY=2g \
                "$JUNTO_CI_NODE_IMAGE" sh -c 'cd recording-layout && npm ci --ignore-scripts &&
                    npx --no-install playwright install-deps chromium && cd /src &&
                    exec tools/linux/run-as-user.sh 1000 1000 /src /cache -- tools/ci/layout-page.sh'  ;;
        macos:rust-macos)
            CARGO_TARGET_DIR="$directory/cargo-target" JUNTO_TEST_POSTGRES_DOWNLOADS="$directory/postgres" \
                tools/ci/rust-other-systems.sh ;;
        macos:deployment)
            tools/ci/deployment.sh ;;
        windows:rust-windows)
            rust_windows ;;
        *)
            echo "no job $1 on the $runner runner" >&2
            return 2 ;;
    esac
}

if [ -n "${JUNTO_CI_JOB:-}" ]; then
    job "$JUNTO_CI_JOB"
    exit
fi

now() { date -u +%Y-%m-%dT%H:%M:%SZ; }
caches_check before
# The shell of the job part, for group.py, which is a native program on Windows.
shell=$(command -v sh)
if [ "$runner" = windows ]; then shell=$(cygpath -w "$shell"); fi
failed=0
for name in "$@"; do
    start=$(now)
    began=$(date +%s)
    printf '%s\t%s\n' "$name" "$start" >"$here/current"
    # The job runs in the background in a group of its own, so that a signal to this script
    # ends it and every process that it started. It does not see the terminal of the runner.
    group=junto-ci-$run-$name
    JUNTO_CI_JOB=$name python3 "$checkout/tools/ci/group.py" "$group" \
        "$shell" "$0" "$directory" "$run" "$runner" "$repository" "$name" </dev/null >"$here/logs/$name.log" 2>&1 &
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
