#!/bin/sh
# Fails when a version that two files state differs between them, or when an image, a CI
# action, a Debian package of an image definition or a package that a workflow installs is
# not pinned (ADR 0014, ADR 0905). Run from any directory.
set -eu
cd "$(dirname "$0")/.."

failures=0
digest='@sha256:[0-9a-f]\{64\}'
fail() {
    echo "FAIL  $1"
    failures=$((failures + 1))
}
same() {
    if [ "$2" = "$3" ] && [ -n "$2" ]; then
        echo "ok    $1: $2"
    else
        fail "$1: \"$2\" and \"$3\""
    fi
}

server=deploy/images/server/Dockerfile
linux=tools/linux/Dockerfile
worker=workers/transcription/docker/Dockerfile
speech=workers/transcription/tests/speech/Dockerfile
argument() {
    sed -n "s/^ARG $2=//p" "$1" | head -n 1
}

toolchain=$(sed -n 's/^channel = "\(.*\)"$/\1/p' rust-toolchain.toml)
image_rust=$(argument "$server" RUST_IMAGE | sed -n 's/^rust:\([0-9.]*\)-.*/\1/p')
same "Rust version of rust-toolchain.toml and of the server image" "$toolchain" "$image_rust"
same "Rust version of rust-toolchain.toml and of the Linux build image" "$toolchain" \
    "$(sed -n 's/^FROM rust:\([0-9.]*\)-.*/\1/p' "$linux")"
same "FFmpeg version of the server image and of the worker image" \
    "$(argument "$server" FFMPEG_VERSION)" "$(argument "$worker" FFMPEG_VERSION)"
same "FFmpeg checksum of the server image and of the worker image" \
    "$(argument "$server" FFMPEG_SHA256)" "$(argument "$worker" FFMPEG_SHA256)"

deployed_livekit=$(sed -n 's/^ *image: livekit\/livekit-server:\(v[0-9.]*\)@.*/\1/p' deploy/base.yaml)
tested_livekit=$(sed -n 's/.*livekit\/livekit-server:\(v[0-9.]*\).*/\1/p' tools/with-test-livekit.sh | head -n 1)
same "LiveKit version of the deployment and of the tests" "$deployed_livekit" "$tested_livekit"
deployed_postgres=$(sed -n 's/^ *image: postgres:\([0-9]*\)\..*/\1/p' deploy/base.yaml)
tested_postgres=$(sed -n 's/.*:-postgres:\([0-9]*\)-.*/\1/p' tools/with-test-postgres.sh | head -n 1)
same "PostgreSQL major version of the deployment and of the tests" "$deployed_postgres" "$tested_postgres"

# The CI jobs read their images and tool versions from tools/ci/pins.sh (ADR 0905).
pins=tools/ci/pins.sh
pin() {
    sed -n "s/^: \"\${$1:=\(.*\)}\"$/\1/p; s/^$1=\(.*\)$/\1/p" "$pins" | head -n 1
}
same "PostgreSQL image of the deployment and of the tests in CI" \
    "$(sed -n 's/^ *image: \(postgres:.*\)$/\1/p' deploy/base.yaml)" "$(pin JUNTO_TEST_POSTGRES_IMAGE)"
same "LiveKit image of the deployment and of the tests in CI" \
    "$(sed -n 's/^ *image: \(livekit\/livekit-server:.*\)$/\1/p' deploy/base.yaml)" "$(pin JUNTO_TEST_LIVEKIT_IMAGE)"
unpinned=$(grep -n '_IMAGE[:=]' "$pins" | grep -v "$digest" || true)
if [ -n "$unpinned" ]; then
    echo "$unpinned"
    fail "$pins names an image without a digest"
else
    echo "ok    every image of $pins has a digest"
fi
same "Node version of $pins and of its Node image" \
    "$(pin JUNTO_CI_NODE_VERSION)" "$(pin JUNTO_CI_NODE_IMAGE | sed -n 's/^node:\([0-9.]*\)-.*/\1/p')"
same "uv version of $pins and of its uv image" \
    "$(pin JUNTO_CI_UV_VERSION)" "$(pin JUNTO_CI_UV_IMAGE | sed -n 's/^[^:]*:\([0-9.]*\)@.*/\1/p')"
same "Python version of the transcription worker and of the Python image of $pins" \
    "$(sed -n 's/^requires-python = "==\([0-9.]*\)\.\*"$/\1/p' workers/transcription/pyproject.toml)" \
    "$(pin JUNTO_CI_PYTHON_IMAGE | sed -n 's/^python:\([0-9]*\.[0-9]*\)\..*/\1/p')"

# A container that a CI script starts or creates names its image by a variable of
# tools/ci/pins.sh or by a digest. A command that continues over several lines is read whole.
unpinned=$(for file in tools/ci/*.sh; do
    awk '
        { instruction = instruction $0 }
        /\\$/ { sub(/\\$/, "", instruction); next }
        { print FILENAME ": " instruction; instruction = "" }
    ' "$file"
done | grep -E 'docker (run|create) ' | grep -v -E '"\$JUNTO_[A-Z_]*_IMAGE"|@sha256:[0-9a-f]{64}' || true)
if [ -n "$unpinned" ]; then
    echo "$unpinned"
    fail "a CI script starts a container whose image is neither a variable of $pins nor named by a digest"
else
    echo "ok    every container of the CI scripts has an image of $pins or a digest"
fi
# A CI script installs a package of the runner with apt-get only with its version.
other=$(grep -n -E '(apt-get|apt|aptitude|dpkg|snap)( +-[^ ]+)* +(install|-i)' tools/ci/*.sh \
    | grep -v -E 'apt-get install [^\\]*$' || true)
unpinned=$(grep -h 'apt-get install' tools/ci/*.sh | sed 's/.*apt-get install//' | tr -s ' ' '\n' \
    | grep -v -E -e '^$' -e '^-' -e '^[a-z0-9][a-z0-9+.-]*=[0-9][0-9A-Za-z.+~:-]*$' || true)
if [ -n "$other$unpinned" ]; then
    echo "$other$unpinned"
    fail "a CI script installs a package of the runner without a version, or in a form that this check does not read"
else
    echo "ok    every package that a CI script installs with apt-get has a version"
fi
same "PostgreSQL version of the tests in CI on Linux and on Windows" \
    "$(pin JUNTO_TEST_POSTGRES_IMAGE | sed -n 's/^postgres:\([0-9.]*\)-.*/\1/p')" \
    "$(sed -n 's/^VERSION = "\([0-9.]*\)-[0-9]*"$/\1/p' tools/windows/with_test_postgres.py)"
same "PostgreSQL version of the tests in CI on Linux and on macOS" \
    "$(pin JUNTO_TEST_POSTGRES_IMAGE | sed -n 's/^postgres:\([0-9.]*\)-.*/\1/p')" \
    "$(pin JUNTO_CI_POSTGRES_BUILD | sed -n 's/^\([0-9.]*\)-[0-9]*$/\1/p')"
# The macOS job downloads the PostgreSQL build for macOS (ADR 0916); the program that downloads
# it checks the SHA-256 of the file against tools/ci/pins.sh.
if pin JUNTO_CI_POSTGRES_MACOS_SHA256 | grep -qx '[0-9a-f]\{64\}' \
    && grep -q 'sha256_of(partial)' tools/with_native_postgres.py; then
    echo "ok    the PostgreSQL download of the macOS tests is checked by its SHA-256"
else
    fail "tools/with_native_postgres.py downloads PostgreSQL without a SHA-256 in $pins"
fi
# The tools of the Windows CI machine (ADR 0905): each download of the setup has a SHA-256 of
# its own and is fetched through the function that checks it; its Python has the minor
# version of the Python image.
setup=tools/windows/ci-setup.ps1
tools=$(sed -n "s/^\\\$\([A-Za-z]*\)Url = 'https:\/\/[^']*'$/\1/p" "$setup")
unchecked=""
for tool in $tools; do
    grep -q "^\\\$${tool}Sha256 = '[0-9a-f]\{64\}'$" "$setup" \
        && grep -q "Get-Pinned \\\$${tool}Url \\\$${tool}Sha256" "$setup" || unchecked="$unchecked $tool"
done
if [ -z "$tools" ] || [ -n "$unchecked" ] || [ "$(grep -c "Url = '" "$setup")" != "$(printf '%s\n' "$tools" | wc -l | tr -d ' ')" ]; then
    fail "$setup downloads a tool without checking a SHA-256 of its own:${unchecked:- a download in a form that this check does not read}"
else
    echo "ok    every download of $setup is checked by its SHA-256:" $tools
fi
# rustup replaces itself with its latest release unless its self-update is off: the setup
# turns it off, and the job installs the Rust version without it.
if grep -qF "Invoke-Rustup \$rustup @('set', 'auto-self-update', 'disable')" "$setup" \
    && grep -q 'rustup toolchain install .*--no-self-update' tools/ci/run-jobs.sh \
    && ! grep 'rustup toolchain install' tools/ci/run-jobs.sh | grep -qv -- '--no-self-update'; then
    echo "ok    rustup of the Windows CI machine does not update itself"
else
    fail "rustup of the Windows CI machine can update itself: $setup must run \"rustup set auto-self-update disable\" and every \"rustup toolchain install\" of tools/ci/run-jobs.sh needs --no-self-update"
fi
same "Python version of the Windows CI machine and of the Python image of $pins" \
    "$(sed -n "s/^\\\$PythonVersion = '\([0-9]*\.[0-9]*\)\.[0-9]*'$/\1/p" "$setup")" \
    "$(pin JUNTO_CI_PYTHON_IMAGE | sed -n 's/^python:\([0-9]*\.[0-9]*\)\..*/\1/p')"

if [ -d .github/workflows ]; then
    same "Node version of the workflow and of $pins" \
        "$(sed -n 's/^ *node-version: \([0-9.]*\)$/\1/p' .github/workflows/ci.yml)" "$(pin JUNTO_CI_NODE_VERSION)"
    same "uv version of the workflow and of $pins" \
        "$(sed -n 's/.*pipx install uv==\([0-9.]*\)$/\1/p' .github/workflows/ci.yml)" "$(pin JUNTO_CI_UV_VERSION)"
    same "cargo-deny version of the workflow cache and of $pins" \
        "$(sed -n 's/^ *key: cargo-deny-\([0-9.]*\)-.*/\1/p' .github/workflows/ci.yml)" "$(pin JUNTO_CI_CARGO_DENY_VERSION)"
    # The Windows job downloads the PostgreSQL build for Windows (ADR 0912); the program
    # that downloads it checks the SHA-256 of the file.
    if grep -q '^SHA256 = "[0-9a-f]\{64\}"$' tools/windows/with_test_postgres.py \
        && grep -q 'sha256_of(partial)' tools/windows/with_test_postgres.py; then
        echo "ok    the PostgreSQL download of the Windows tests is checked by its SHA-256"
    else
        fail "tools/windows/with_test_postgres.py downloads PostgreSQL without checking its SHA-256"
    fi
fi

unpinned=$(grep -n '^ *image:' deploy/*.yaml | grep -v -F -e 'image: ${JUNTO_SERVER_IMAGE:' -e 'image: ${JUNTO_INIT_IMAGE:' | grep -v "$digest" || true)
if [ -n "$unpinned" ]; then
    echo "$unpinned"
    fail "a Compose file names an image without a digest"
else
    echo "ok    every external image of the Compose files has a digest"
fi
for file in "$server" "$worker" "$speech" deploy/images/link/Dockerfile; do
    unpinned=$(grep -n -e '^ARG [A-Z_]*_IMAGE=' -e '^# syntax=' "$file" | grep -v "$digest" || true)
    literal=$(grep -n '^FROM ' "$file" | grep -v -e '\${[A-Z_]*_IMAGE}' -e ' AS [a-z-]*$' | grep -v "$digest" || true)
    stages=$(grep -n '^FROM [a-z][a-z0-9.-]*[:/]' "$file" | grep -v "$digest" || true)
    if [ -n "$unpinned$literal$stages" ]; then
        echo "$unpinned$literal$stages"
        fail "$file names an image without a digest"
    else
        echo "ok    every base image of $file has a digest"
    fi
done

# A Debian package is pinned by the dated snapshot of the archive that it comes from
# (ADR 0014). In these two definitions, an instruction that names a package program has
# one form only, so that this rule reads all of it: it removes the package sources of the
# base image, writes the two sources of the snapshot, and installs packages by their names
# without a further option. The snapshot is an argument of the same build stage. An
# instruction that names the configuration of the package program in another way fails.
same "Debian snapshot of the server image and of the speech image" \
    "$(argument "$server" DEBIAN_SNAPSHOT)" "$(argument "$speech" DEBIAN_SNAPSHOT)"
for file in "$server" "$speech"; do
    unpinned=$(awk '
        { instruction = instruction $0 }
        /\\$/ { sub(/\\$/, "", instruction); next }
        { gsub(/[ \t]+/, " ", instruction); print instruction; instruction = "" }
    ' "$file" | awk '
        /^FROM / { dated = 0 }
        /^ARG DEBIAN_SNAPSHOT=[0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]T[0-9][0-9][0-9][0-9][0-9][0-9]Z$/ { dated = 1 }
        /^ARG DEBIAN_SNAPSHOT/ { next }
        tolower($0) !~ /(apt|dpkg|sources\.list)/ { next }
        dated && /^RUN rm -f \/etc\/apt\/sources\.list\.d\/\* && echo "deb \[check-valid-until=no\] http:\/\/snapshot\.debian\.org\/archive\/debian\/\$\{DEBIAN_SNAPSHOT\} [a-z]+ main" > \/etc\/apt\/sources\.list && echo "deb \[check-valid-until=no\] http:\/\/snapshot\.debian\.org\/archive\/debian-security\/\$\{DEBIAN_SNAPSHOT\} [a-z]+-security main" >> \/etc\/apt\/sources\.list && apt-get update && apt-get install -y --no-install-recommends( [a-z0-9][a-z0-9.+-]*)+ && rm -rf \/var\/lib\/apt\/lists\/\*$/ { next }
        { print }
    ')
    if [ -n "$unpinned" ]; then
        echo "$unpinned"
        fail "$file installs a Debian package in another form than from the dated snapshot of its build stage"
    else
        echo "ok    every Debian package of $file comes from the dated snapshot of its build stage"
    fi
done

# The Linux build image, in which the Rust steps of CI on Linux run (ADR 0750): its base image
# has a digest, and each word after "apt-get install" that is not an option names a package
# with one exact version of the dated snapshot that the image reads.
unpinned=$(grep -n '^FROM ' "$linux" | grep -v "$digest" || true)
if [ -n "$unpinned" ]; then
    echo "$unpinned"
    fail "$linux names an image without a digest"
else
    echo "ok    the base image of $linux has a digest"
fi
unpinned=$(awk '
    { instruction = instruction $0 }
    /\\$/ { sub(/\\$/, "", instruction); next }
    { print instruction; instruction = "" }
' "$linux" | awk '
    {
        listing = 0
        for (i = 1; i <= NF; i++) {
            if ($i == "install" && $(i - 1) == "apt-get") { listing = 1; continue }
            if (!listing) continue
            if ($i == "&&") { listing = 0; continue }
            if ($i == "-t") { i++; continue }
            if ($i ~ /^-/) continue
            if ($i !~ /^[a-z0-9][a-z0-9+.-]*=[0-9][0-9A-Za-z.+~:-]*$/) print $i
        }
    }
')
if [ -n "$unpinned" ]; then
    echo "$unpinned"
    fail "$linux installs a Debian package without a version"
else
    echo "ok    every Debian package of $linux has a version"
fi

# A file that the Linux build image downloads from another place than the Debian snapshot is
# checked by its SHA-256 in the same instruction: the instruction names a sum for each address
# and runs sha256sum -c.
unpinned=$(awk '
    { instruction = instruction $0 }
    /\\$/ { sub(/\\$/, "", instruction); next }
    { print instruction; instruction = "" }
' "$linux" | grep -E 'https?://' | grep -v 'snapshot\.debian\.org/archive' | awk '
    BEGIN { for (i = 0; i < 64; i++) sum = sum "[0-9a-f]" }
    {
        addresses = gsub(/https?:\/\//, "&")
        sums = gsub("(^|[^0-9a-f])" sum "([^0-9a-f]|$)", "&")
        if (sums < addresses || $0 !~ /sha256sum (--[a-z]+ )*-c/) print
    }
' || true)
if [ -n "$unpinned" ]; then
    echo "$unpinned"
    fail "$linux downloads a file without checking its SHA-256"
else
    echo "ok    every download of $linux is checked by its SHA-256"
fi

# The words after "pip install" that are not options: each names a package with one exact
# version.
unpinned=$(sed -n '/pip install/,/[^\\]$/p' "$speech" | tr -s ' \\' '\n\n' \
    | grep -v -E -e '^$' -e '^-' -e '^(RUN|&&|pip|install)$' -e '^[A-Za-z0-9_.-]+==[0-9][0-9A-Za-z.+!-]*$' || true)
if [ -n "$unpinned" ]; then
    echo "$unpinned"
    fail "$speech installs a Python package without a version"
else
    echo "ok    every Python package of $speech has a version"
fi

unpinned=$(grep -n '_image=[a-z]' deploy/tests/*.sh | grep -v "$digest" || true)
if [ -n "$unpinned" ]; then
    echo "$unpinned"
    fail "a test of the deployment names an image without a digest"
else
    echo "ok    every image of the deployment tests has a digest"
fi

if [ -d .github/workflows ]; then
    unpinned=$(grep -n 'uses:' .github/workflows/*.yml | grep -v 'uses: \./' | grep -v '@[0-9a-f]\{40\} ' || true)
    if [ -n "$unpinned" ]; then
        echo "$unpinned"
        fail "a workflow uses an action without a commit hash"
    else
        echo "ok    every action of the workflows has a commit hash"
    fi
    unpinned=$(grep -n '[a-z0-9]/[a-z0-9-]*:[a-z0-9][a-z0-9.]*' .github/workflows/*.yml | grep -v 'uses:' | grep -v "$digest" || true)
    if [ -n "$unpinned" ]; then
        echo "$unpinned"
        fail "a workflow runs an image without a digest"
    else
        echo "ok    every image of the workflows has a digest"
    fi
    # A package of the runner is installed in one form only, "apt-get install" with its
    # options and packages on one line, so that this rule reads every package: each word
    # after it that is not an option names a package with one exact version.
    other=$(grep -n -E '(apt-get|apt|aptitude|dpkg|snap)( +-[^ ]+)* +(install|-i)' .github/workflows/* \
        | grep -v -E 'apt-get install [^\\]*$' || true)
    unpinned=$(grep -h 'apt-get install' .github/workflows/* | sed 's/.*apt-get install//' | tr -s ' ' '\n' \
        | grep -v -E -e '^$' -e '^-' -e '^[a-z0-9][a-z0-9+.-]*=[0-9][0-9A-Za-z.+~:-]*$' || true)
    if [ -n "$other$unpinned" ]; then
        echo "$other$unpinned"
        fail "a workflow installs a package of the runner without a version, or in a form that this check does not read"
    else
        echo "ok    every package that a workflow installs with apt-get has a version"
    fi
    other=$(find .github/workflows -type f ! -name '*.yml')
    if [ -n "$other" ]; then
        echo "$other"
        fail "a workflow file has another ending than .yml, and the rules above do not read it"
    else
        echo "ok    every workflow file is read by these rules"
    fi
fi

if [ "$failures" -eq 0 ]; then
    echo "all checks passed"
else
    echo "$failures check(s) failed"
    exit 1
fi
