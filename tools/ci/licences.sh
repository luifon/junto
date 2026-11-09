#!/bin/sh
# The job "Licences of the Rust dependencies" (ADR 0905): cargo deny of the version of
# tools/ci/pins.sh. JUNTO_CI_TOOLS names the directory that keeps the installed tool between
# runs (default: the Cargo home); the tool is installed there when it has another version.
set -eu
cd "$(dirname "$0")/../.."
. tools/ci/pins.sh

tools=${JUNTO_CI_TOOLS:-${CARGO_HOME:-$HOME/.cargo}}
deny="$tools/bin/cargo-deny"
if [ "$("$deny" --version 2>/dev/null || true)" != "cargo-deny $JUNTO_CI_CARGO_DENY_VERSION" ]; then
    printf '\n=== Install cargo-deny %s\n' "$JUNTO_CI_CARGO_DENY_VERSION"
    cargo install --locked --force --root "$tools" "cargo-deny@$JUNTO_CI_CARGO_DENY_VERSION"
fi
# cargo deny reads whether a package is yanked from the index cache of the Cargo home, and the
# cargo fetch that it runs first does not renew the entries of packages that are already there.
# With a Cargo home kept from earlier runs, a version yanked after its entry was cached passed
# (ADR 0905). A resolution without the lock file asks the registry for every package again; the
# dry run writes no lock file.
printf '\n=== The index entries of every package from the registry\n'
cargo update --dry-run --quiet
printf '\n=== Licences, sources and advisories\n'
"$deny" check
