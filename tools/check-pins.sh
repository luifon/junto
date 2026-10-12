#!/bin/sh
# Fails when a version that two files state differs between them, or when an image, a CI
# action or a package that a workflow installs is not pinned (ADR 0014, ADR 0905). Run from
# any directory.
set -eu
cd "$(dirname "$0")/.."

failures=0
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
worker=workers/transcription/docker/Dockerfile
speech=workers/transcription/tests/speech/Dockerfile
argument() {
    sed -n "s/^ARG $2=//p" "$1" | head -n 1
}

toolchain=$(sed -n 's/^channel = "\(.*\)"$/\1/p' rust-toolchain.toml)
image_rust=$(argument "$server" RUST_IMAGE | sed -n 's/^rust:\([0-9.]*\)-.*/\1/p')
same "Rust version of rust-toolchain.toml and of the server image" "$toolchain" "$image_rust"
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

if [ -d .github/workflows ]; then
    same "PostgreSQL image of the deployment and of the tests in CI" \
        "$(sed -n 's/^ *image: \(postgres:.*\)$/\1/p' deploy/base.yaml)" \
        "$(sed -n 's/^ *JUNTO_TEST_POSTGRES_IMAGE: \(.*\)$/\1/p' .github/workflows/ci.yml)"
    same "LiveKit image of the deployment and of the tests in CI" \
        "$(sed -n 's/^ *image: \(livekit\/livekit-server:.*\)$/\1/p' deploy/base.yaml)" \
        "$(sed -n 's/^ *JUNTO_TEST_LIVEKIT_IMAGE: \(.*\)$/\1/p' .github/workflows/ci.yml)"
fi

digest='@sha256:[0-9a-f]\{64\}'
unpinned=$(grep -n '^ *image:' deploy/*.yaml | grep -v "$digest" || true)
if [ -n "$unpinned" ]; then
    echo "$unpinned"
    fail "a Compose file names an image without a digest"
else
    echo "ok    every image of the Compose files has a digest"
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
