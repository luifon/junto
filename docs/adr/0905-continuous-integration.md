# 0905. Continuous integration workflow

## Status

Accepted

The tests that need PostgreSQL do not run on macOS. On Windows the window tests of the client run against PostgreSQL since ADR 0912; the other tests that need PostgreSQL do not run there.

## Context

ADR 0902 decides that CI builds and tests on macOS, Windows and Linux for every commit, regenerates the API documentation and checks the licence of every dependency (ADR 0801, ADR 0202). The repository has a Rust workspace, the recording layout page with browser tests (ADR 0609), the transcription worker in Python (ADR 0610), and the container images and Compose files (ADR 0014, ADR 0015). The integration tests need a PostgreSQL that the test script starts in Docker (ADR 0904); the runners for macOS and Windows have no Docker for Linux containers. A workflow that names an action by a tag runs other code when the tag moves.

## Decision

- One workflow, `.github/workflows/ci.yml`, runs for every push and for every pull request from another repository.
- Every action is named by its commit hash, with the version as a comment. Every container image that a step runs is named by its digest: the workflow gives the two test scripts the PostgreSQL image and the LiveKit image of the deployment by digest. `tools/check-pins.sh` fails when one is not, and when those two differ from the images of the deployment.
- A package that a step installs with `apt-get` is named with its version. `tools/check-pins.sh` fails for an `apt-get install` without a version in a workflow. Not pinned: the programs of the runner image and the system packages that Playwright installs for its browser.
- The Rust steps of the Linux job run in the Linux build image of ADR 0750 (`tools/linux/Dockerfile`), through `tools/linux-container.sh`. The image is the one list of system packages for the local Linux checks and for CI: the libraries of the window, of sound and of screen capture, clang 22, which the call library needs (Ubuntu 24.04 has clang 20 at most, and ADR 0750 rejects the package repository of the LLVM project as a pinned source), PipeWire for the tests of sound, and `ffmpeg` for the tests of recordings. Its base image has a digest, and each package has a version from one dated Debian snapshot; `tools/check-pins.sh` checks both, and that its Rust version is the one of `rust-toolchain.toml`. Each command of the job gets its own PostgreSQL and LiveKit server in Docker; Cargo's registry and target directory stay in Docker volumes from one step to the next.
- The workflow has no permission by default. Each job gets `contents: read`. No job uses a secret, and the checkout keeps no credential.
- The runner images are named by version, not by `latest`.
- The Rust version is the one of `rust-toolchain.toml` (ADR 0014).

Jobs:

- Rust on Linux: the free disk space of the runner as in the deployment job, the build of the Linux image, then in it `cargo fmt --check`, `cargo clippy` with warnings as errors and with the bound on the stack of one function (ADR 0906), the tests of the workspace with the private PostgreSQL of the container runner; on the runner the packaging unit tests; in the image the check of the Linux packages (ADR 0754); then the generation of the API documents in the image, followed on the runner by a comparison with the committed files.
- Rust on macOS and on Windows: a build of every target of the workspace, and the tests of `junto-authz`, `junto-protocol` and the library and tools of `junto-client-core`, which need no database. On macOS also the check of the condition waits (ADR 0786): `tools/check-condition-wait.sh` with the built client binary, and the unit tests of that module of `junto-media`. On macOS the signing, update and recovery checks of test bundles (ADR 0725, ADR 0726). On Windows every crate is built with the static C runtime (ADR 0753), and the check of the Windows package installs, starts, updates and rolls back the ZIP (ADR 0755). On Windows also the window tests of the client, without and with PostgreSQL (ADR 0912).
- Licences: `cargo deny check` with a stated version of the tool.
- Recording layout page: the locked packages without install scripts, the build, and the browser tests against a LiveKit server in Docker.
- Transcription worker: the locked packages with `uv`, `ruff`, `mypy`, and the unit tests.
- Images and deployment: the check of pinned versions, the linter for workflow files and the linter for shell scripts from pinned images, the rendered Compose files of every form, the builds of the server image and the link image, the test of the hand-over scripts, the tests of the recorder start script and of the entry, the start of the local form, which builds the worker image, a backup of a running server with a restore into an empty deployment (ADR 0902, ADR 0911), an upgrade from an earlier release with the way back (ADR 0911), and the start of the one-host form and of the two-host form on the runner. This job fetches the whole history, because the upgrade test reads the files of an earlier commit.

- The deployment job sets every Compose CPU limit variable from the runner's available processor count before starting any form. Deployment defaults stay unchanged; a recorder or worker default larger than the runner must not make container creation fail.

- `tools/serial.sh` uses `flock` where `lockf` does not exist, so that the scripts that call it run on Linux.

## Consequences

- An error that exists only with PostgreSQL on macOS or Windows is not found by CI. ADR 0904 names a PostgreSQL for those runners as open.
- The tier of the call tests against a real LiveKit server, the tier with the real worker and its models, and the mutation tests of the layout page do not run in CI. They need a build of the test participant, the model files, and more time than a commit check has.
- The recording test of the deployment (`deploy/tests/recording.sh`, ADR 0910) does not run in CI. The storage guard starts a recording only with 13 GiB of free disk space (ADR 0605), and a hosted runner states 14 GB of disk, of which the images of the deployment take 5.3 GB without the cache of their builds. The recorder needs up to 3 CPU cores for the composite while the media server, the server and three test participants run, and a hosted runner has 4; a recorder that is short of CPU writes a file with holes (ADR 0602), so the test would fail without a defect. CI lints the scripts of the test.
- The licences of the Python packages and of the packages of the layout page are not checked by a tool in CI. The build of the layout page fails when a bundled package has no licence file, and `THIRD-PARTY.md` of the worker is kept by hand (ADR 0610).
- The Linux packages and the Windows package of each commit are built from debug builds of the client and checked; release builds and their publication are not part of this workflow. The macOS bundle of the product is checked by the manual release check of `packaging/check_release.py`.
- The Linux job builds the Linux image in every run; no cache keeps the image or the target directory between runs, so every run compiles the workspace. The runner of a private repository has two processors; the container runner starts one Cargo job for each processor of its set.
- The Linux image reads its packages from `snapshot.debian.org`. The job fails while that service cannot be reached.
- A new version of an action is a change of its commit hash in the workflow file.
- `cargo deny` is compiled in its job when the cache has no copy.
