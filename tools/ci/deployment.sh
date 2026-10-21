#!/bin/sh
# The job "Images and the Compose deployment" (ADR 0905). Needs Docker with Compose, and a
# clone with the whole history: deploy/tests/upgrade.sh reads the files of an earlier commit.
# The server refuses new files while its disk has 5 GiB or less of free space (ADR 0605),
# and the images of this job take several GB.
set -eu
cd "$(dirname "$0")/../.."
. tools/ci/pins.sh

step() { printf '\n=== %s\n' "$1"; }

# Each Compose CPU limit variable of deploy/base.yaml gets at most the processor count of
# the Docker host, so that a recorder or worker default larger than the host does not make
# container creation fail. The deployment defaults stay as they are.
step "Fit Compose CPU limits to the Docker host"
cpus=$(docker info --format '{{.NCPU}}')
settings=$(sed -n 's/^[[:space:]]*cpus: ${\(JUNTO_[A-Z_]*_CPUS\):-\([0-9][0-9]*\)}/\1 \2/p' deploy/base.yaml)
while read -r setting limit; do
    [ -n "$setting" ] || continue
    if [ "$limit" -gt "$cpus" ]; then limit=$cpus; fi
    echo "$setting=$limit"
    export "$setting=$limit"
done <<EOF
$settings
EOF
# The images of this job are built by a BuildKit builder of its own, in a container of the
# Docker daemon. tools/ci/caches.py measures its cache and removes it above its cap; the
# builder of the Docker daemon is shared with the other work of the machine and is not pruned
# (ADR 0905). Compose and docker build use the builder that BUILDX_BUILDER names; docker
# build needs --load to place the image in the image store of the daemon.
step "BuildKit builder of the job"
builder=junto-ci-deployment
if docker buildx inspect "$builder" >/dev/null 2>&1 \
    && ! docker buildx inspect "$builder" | grep -q "image=\"$JUNTO_CI_BUILDKIT_IMAGE\""; then
    docker buildx rm "$builder"
fi
if ! docker buildx inspect "$builder" >/dev/null 2>&1; then
    docker buildx create --name "$builder" --driver docker-container \
        --driver-opt "image=$JUNTO_CI_BUILDKIT_IMAGE"
fi
docker buildx inspect --bootstrap "$builder" >/dev/null
export BUILDX_BUILDER="$builder"
step "Pinned versions agree"
tools/check-pins.sh
step "Workflow files"
docker run --rm --network none -v "$PWD:/repository:ro" -w /repository "$JUNTO_CI_ACTIONLINT_IMAGE" -color
step "Shell scripts"
docker run --rm --network none -v "$PWD:/repository:ro" -w /repository "$JUNTO_CI_SHELLCHECK_IMAGE" \
    --severity=warning --external-sources \
    tools/serial.sh tools/check-pins.sh tools/with-test-postgres.sh tools/with-test-livekit.sh \
    tools/linux-container.sh tools/linux/run.sh tools/linux/session.sh tools/linux/run-as-user.sh tools/linux/display.sh \
    tools/linux/appimage-runtime/build.sh tools/linux/appimage-runtime/link.sh \
    tools/ci/pins.sh tools/ci/ci.sh tools/ci/run-jobs.sh tools/ci/rust-linux.sh \
    tools/ci/rust-other-systems.sh tools/ci/licences.sh tools/ci/layout-page.sh \
    tools/ci/transcription-worker.sh tools/ci/deployment.sh \
    packaging/release.sh packaging/release_locked.sh packaging/release_linux.sh packaging/sign.sh \
    deploy/generate-secrets.sh deploy/backup.sh deploy/restore.sh deploy/upgrade-undo.sh \
    deploy/database-user.sh deploy/check-limits.sh \
    deploy/images/server/junto-transcribe-handover \
    deploy/images/server/junto-volumes deploy/images/worker/junto-worker-loop \
    deploy/images/recorder/start.sh deploy/images/link/entrypoint.sh \
    deploy/tests/compose-config.sh deploy/tests/smoke.sh deploy/tests/entry.sh \
    deploy/tests/one-host.sh deploy/tests/two-hosts.sh deploy/tests/backup-restore.sh \
    deploy/tests/upgrade.sh \
    deploy/tests/recorder-start.sh deploy/tests/recording.sh deploy/tests/recording-prepare.sh \
    deploy/tests/transcribe-handover.sh deploy/tests/transcribe-handover-inside.sh
step "Compose files of every form"
deploy/tests/compose-config.sh
step "Unit tests of the deployment checks"
python3 -B -m unittest discover -s deploy/tests -p 'test_*.py'
step "Server image"
docker build --load -f deploy/images/server/Dockerfile -t junto-ci-server .
step "The licence files of the server image"
python3 -B deploy/tests/check_licences.py junto-ci-server
step "Link image"
docker build --load -t junto-ci-link deploy/images/link
step "Hand-over of a transcription job between two containers"
deploy/tests/transcribe-handover.sh junto-ci-server
step "The start script of the recorder saves partial files"
deploy/tests/recorder-start.sh junto-ci-server
step "The entry selects by server name"
deploy/tests/entry.sh junto-ci-entry
step "The local form starts, with the worker image"
deploy/tests/smoke.sh junto-ci
step "Backup of a running server, and restore into an empty deployment"
deploy/tests/backup-restore.sh junto-ci-backup
step "Upgrade from an earlier release, and the way back"
deploy/tests/upgrade.sh junto-ci-upgrade
step "The one-host form starts behind the entry"
deploy/tests/one-host.sh junto-ci-one
step "The two-host form starts as two projects joined by the link"
deploy/tests/two-hosts.sh junto-ci-two
