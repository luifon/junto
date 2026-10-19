# shellcheck shell=sh disable=SC2034
# The images and tool versions of the CI jobs (ADR 0905). The job scripts and the runner
# read this file with ".", and tools/check-pins.sh compares it with the deployment and the
# workflow. Each image is named by its digest. A value set by the caller is kept.

# The images that tools/with-test-postgres.sh and tools/with-test-livekit.sh start: the
# images of deploy/base.yaml.
: "${JUNTO_TEST_POSTGRES_IMAGE:=postgres:17.11-alpine3.24@sha256:b0f9560a2de083e2cc7382e75f808c7381a32852a7ec49117deedb300e552b24}"
: "${JUNTO_TEST_LIVEKIT_IMAGE:=livekit/livekit-server:v1.13.9@sha256:d0c04791bf63ca8dcea123571827d56ba9504bd57c8d5de60ee03b5408cc3154}"
export JUNTO_TEST_POSTGRES_IMAGE JUNTO_TEST_LIVEKIT_IMAGE

# The linters of the deployment job.
JUNTO_CI_ACTIONLINT_IMAGE=rhysd/actionlint:1.7.12@sha256:b1934ee5f1c509618f2508e6eb47ee0d3520686341fec936f3b79331f9315667
# The BuildKit of the builder of the deployment job (tools/ci/deployment.sh).
JUNTO_CI_BUILDKIT_IMAGE=moby/buildkit:v0.27.1@sha256:1e110c71d389d6d24f67b9438e2f7b8da749a6ff407b22a1631e025c95599368
JUNTO_CI_SHELLCHECK_IMAGE=koalaman/shellcheck:v0.11.0@sha256:61862eba1fcf09a484ebcc6feea46f1782532571a34ed51fedf90dd25f925a8d

# The tool versions that the jobs install when the runner does not provide them.
JUNTO_CI_CARGO_DENY_VERSION=0.20.2
JUNTO_CI_UV_VERSION=0.11.13
JUNTO_CI_NODE_VERSION=22.23.3

# The images in which a Linux host without these tools runs the jobs (tools/ci/run-jobs.sh).
# The Node and Python images state the versions above; the uv image and the Docker CLI image
# provide one program each, which the runner copies into its tools volume.
JUNTO_CI_NODE_IMAGE=node:22.23.3-bookworm@sha256:0e5f906573693feaa1e21057ebdcfdb5bd5021f050b2dc7c9deceb629c7da2a8
JUNTO_CI_PYTHON_IMAGE=python:3.12.14-slim-bookworm@sha256:392307d22300de8b5986851a12d9176dfc0fc073e65bf6523ebd7dcbeb23564e
JUNTO_CI_UV_IMAGE=ghcr.io/astral-sh/uv:0.11.13@sha256:841c8e6fe30a8b07b4478d12d0c608cba6de66102d29d65d1cc423af86051563
JUNTO_CI_DOCKER_CLI_IMAGE=docker:29.4.0-cli@sha256:2efe7c8e806b35050def6ba1ba0ddad67b521a0a6d37f4910b3d88f11b495d03
