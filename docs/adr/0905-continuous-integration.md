# 0905. Continuous integration workflow

## Status

Accepted

The tests that need PostgreSQL do not run on macOS. On Windows the window tests of the client run against PostgreSQL since ADR 0912; the other tests that need PostgreSQL do not run there.

## Context

ADR 0902 decides that CI builds and tests on macOS, Windows and Linux for every commit, regenerates the API documentation and checks the licence of every dependency (ADR 0801, ADR 0202). The repository has a Rust workspace, the recording layout page with browser tests (ADR 0609), the transcription worker in Python (ADR 0610), and the container images and Compose files (ADR 0014, ADR 0015). The integration tests need a PostgreSQL that the test script starts in Docker (ADR 0904); the runners for macOS and Windows have no Docker for Linux containers. A workflow that names an action by a tag runs other code when the tag moves.

## Decision

- The steps of each job are one script under `tools/ci/`: `rust-linux.sh`, `rust-other-systems.sh` (macOS and Windows), `licences.sh`, `layout-page.sh`, `transcription-worker.sh` and `deployment.sh`. The images and tool versions of the jobs are in `tools/ci/pins.sh`. A runner only prepares the tools that a script needs and calls it, so that every runner runs the same steps.
- CI runs on a Linux host of the project over SSH and on a macOS machine (`tools/ci/ci.sh`); the GitHub workflow stays valid and calls the same scripts. See "Runners outside GitHub" below.
- One workflow, `.github/workflows/ci.yml`, runs for every push and for every pull request from another repository.
- Every action is named by its commit hash, with the version as a comment. Every container image that a job runs is named by its digest in `tools/ci/pins.sh`, which gives the two test scripts the PostgreSQL image and the LiveKit image of the deployment. `tools/check-pins.sh` fails when an image there has no digest, when those two differ from the images of the deployment, and when the Node, uv, Python or cargo-deny version of the workflow differs from `tools/ci/pins.sh`.
- A package that a step installs with `apt-get` is named with its version. `tools/check-pins.sh` fails for an `apt-get install` without a version in a workflow or in a script of `tools/ci/`. Not pinned: the programs of the runner image and the system packages that Playwright installs for its browser.
- The Rust steps of the Linux job run in the Linux build image of ADR 0750 (`tools/linux/Dockerfile`), through `tools/linux-container.sh`. The image is the one list of system packages for the local Linux checks and for CI: the libraries of the window, of sound and of screen capture, clang 22, which the call library needs (Ubuntu 24.04 has clang 20 at most, and ADR 0750 rejects the package repository of the LLVM project as a pinned source), PipeWire for the tests of sound, and `ffmpeg` for the tests of recordings. Its base image has a digest, and each package has a version from one dated Debian snapshot; `tools/check-pins.sh` checks both, and that its Rust version is the one of `rust-toolchain.toml`. Each command of the job gets its own PostgreSQL and LiveKit server in Docker; Cargo's registry and target directory stay in Docker volumes from one step to the next.
- The workflow has no permission by default. Each job gets `contents: read`. No job uses a secret, and the checkout keeps no credential.
- The runner images are named by version, not by `latest`.
- The Rust version is the one of `rust-toolchain.toml` (ADR 0014).

Jobs:

- Rust on Linux: the free disk space of the runner as in the deployment job, the build of the Linux image, then in it `cargo fmt --check`, `cargo clippy` with warnings as errors and with the bound on the stack of one function (ADR 0906), the tests of the workspace with the private PostgreSQL of the container runner; on the runner the packaging unit tests; in the image the check of the Linux packages (ADR 0754); then the generation of the API documents in the image, followed on the runner by a comparison with the committed files.
- Rust on macOS and on Windows (`rust-other-systems.sh`): a build of every target of the workspace, and the tests of `junto-authz`, `junto-protocol` and the library and tools of `junto-client-core`, which need no database. On macOS also the check of the condition waits (ADR 0786): `tools/check-condition-wait.sh` with the built client binary, and the unit tests of that module of `junto-media`. On macOS the signing, update and recovery checks of test bundles (ADR 0725, ADR 0726). On Windows every crate is built with the static C runtime (ADR 0753), and the check of the Windows package installs, starts, updates and rolls back the ZIP (ADR 0755). On Windows also the window tests of the client, without and with PostgreSQL (ADR 0912).
- Licences: `cargo deny check` with a stated version of the tool.
- Recording layout page: the locked packages without install scripts, the build, and the browser tests against a LiveKit server in Docker.
- Transcription worker: the locked packages with `uv`, `ruff`, `mypy`, and the unit tests.
- Images and deployment: the check of pinned versions, the linter for workflow files and the linter for shell scripts from pinned images, the rendered Compose files of every form, the builds of the server image and the link image, the test of the hand-over scripts, the tests of the recorder start script and of the entry, the start of the local form, which builds the worker image, a backup of a running server with a restore into an empty deployment (ADR 0902, ADR 0911), an upgrade from an earlier release with the way back (ADR 0911), and the start of the one-host form and of the two-host form on the runner. This job fetches the whole history, because the upgrade test reads the files of an earlier commit.

- The deployment job sets every Compose CPU limit variable from the processor count of the Docker host before starting any form. Deployment defaults stay unchanged; a recorder or worker default larger than the runner must not make container creation fail.

- `tools/serial.sh` uses `flock` where `lockf` does not exist, so that the scripts that call it run on Linux.

### Runners outside GitHub

GitHub Actions cannot be used for this project at present. CI runs on two machines of the project, started from a developer machine with `tools/ci/ci.sh`:

- **Linux host, over SSH.** `ci.sh run <commit>` pushes the commit to a bare repository on the host as the ref `refs/ci/<run>` and starts `tools/ci/run-jobs.sh` of that commit there with `setsid nohup`, so the run continues when the developer machine sleeps or the connection drops. Jobs: Rust on Linux, licences, the transcription worker, the recording layout page. The SSH alias and the CI directory of the host come from `JUNTO_CI_HOST` and `JUNTO_CI_DIR` in `.env.local`, which is not tracked.
- **macOS machine.** `ci.sh local <commit>` runs `run-jobs.sh` on the developer's Mac through `tools/serial.sh`. Jobs: Rust on macOS, and the images with the Compose deployment.
- **Windows.** Not yet. The Windows job will run in a Windows virtual machine in a container named `junto-ci-windows` on the Linux host, with `/dev/kvm` passed into the container and the same processor and memory limits as a Linux job. The VM image is prepared once with Git for Windows, Python, Visual Studio Build Tools and the Rust version of `rust-toolchain.toml`, kept in a volume, and reached over SSH from `run-jobs.sh`, which then runs `tools/ci/rust-other-systems.sh` in a clone of the commit inside the VM. `ci.sh` adds a third runner, `windows`, with the job `rust-windows`. The host needs hardware virtualisation enabled in its firmware first.

`run-jobs.sh` clones the commit, takes the run lock (one run at a time per machine: `tools/serial.sh`, on the Linux host with a lock file of the CI directory), runs the jobs one after the other, and records each job in `runs/<run>/record.tsv`: commit, job, start and end in UTC, exit status, log file, seconds. `runs/<run>/current` names the running job; `runs/<run>/finished` holds the exit status of the run. At the end, also after a failure, it removes the clone and every container and network whose name starts with `junto-ci-`. `ci.sh status <run>` prints each job as passed, failed, running or waiting with its time, and copies the log of a failed job to `target/ci-logs/<run>/`. `ci.sh forget <run>` removes a run, its ref and a commit that no other ref keeps.

**A commit has passed CI** when its latest run on the Linux host and its latest run on the macOS machine have finished with every job of the runner's list passed (`ci.sh status <commit>` prints `passed`). A run of only some jobs does not count. When the Windows runner exists, its run is part of the rule.

On the Linux host, a run has at most 8 processors and 12 GB (11 GiB) of memory over all of its containers, without swap. Jobs run one after the other, so the containers of one job are the containers of the run. Rust on Linux and licences: the build container gets 6 processors as a set (ADR 0750: a quota stops the threads of a test that measures sound in real time) and 8 GiB; PostgreSQL 0.75 processor and 1.5 GiB; LiveKit 1 processor and 768 MiB; the control container 0.25 processor and 128 MiB. The transcription worker: one container of 8 processors and 11 GiB. The layout page: a Node container of 6 processors and 9 GiB, which shares the host's network to reach the LiveKit container of `tools/with-test-livekit.sh` (2 processors, 2 GiB). A host with fewer than 8 processors gives each job its processor count. The uv program and the Docker command-line program come from their images in `tools/ci/pins.sh`, copied into the CI directory. Every Docker object of CI on the host has a name that starts with `junto-ci-`.

The caches of the Linux host are named volumes: `junto-ci-cargo` (Cargo registry, cap 8 GiB), `junto-ci-target` (target directory and the installed cargo-deny, cap 60 GiB), `junto-ci-uv` (uv cache and Python, cap 4 GiB), `junto-ci-npm` (npm cache and the Playwright browser, cap 4 GiB). Docker's local volumes have no size limit, so after each run `run-jobs.sh` measures each volume and empties one that is above its cap; the next run then builds from nothing, as a first run does. On the macOS machine the Cargo target directory of the runner has a cap of 60 GiB with the same rule.

The images and deployment job runs on the macOS machine and not on the Linux host. Its image builds compile the workspace. The builder of the Docker daemon gives the RUN steps of a build no processor or memory limit, and a BuildKit builder in a container, which can be limited, needs a privileged container or seccomp and AppArmor off. The Linux build image of ADR 0750 uses no feature of BuildKit, so on the host `tools/linux/run.sh` builds it with the builder of Docker without BuildKit (`JUNTO_LINUX_BUILDER=limited`): each step runs in a container with the processor set and memory of the build container, and the build leaves no BuildKit cache, which Docker cannot remove for one owner only on a host whose disk other projects share. The image carries the label `junto-linux-image=junto-ci`; an image that a new build replaces is removed by that label.

The images that the jobs pull (`tools/ci/pins.sh`) keep the name of their registry and are run by digest; Docker gives a pulled image no other name unless it is tagged, and a tag would not stop a run by digest from pulling it again. `docker rmi` with each image reference of `tools/ci/pins.sh` removes them.

## Consequences

- An error that exists only with PostgreSQL on macOS or Windows is not found by CI. ADR 0904 names a PostgreSQL for those runners as open.
- The tier of the call tests against a real LiveKit server, the tier with the real worker and its models, and the mutation tests of the layout page do not run in CI. They need a build of the test participant, the model files, and more time than a commit check has.
- The recording test of the deployment (`deploy/tests/recording.sh`, ADR 0910) does not run in CI. The storage guard starts a recording only with 13 GiB of free disk space (ADR 0605), and a hosted runner states 14 GB of disk, of which the images of the deployment take 5.3 GB without the cache of their builds. The recorder needs up to 3 CPU cores for the composite while the media server, the server and three test participants run, and a hosted runner has 4; a recorder that is short of CPU writes a file with holes (ADR 0602), so the test would fail without a defect. CI lints the scripts of the test.
- The licences of the Python packages and of the packages of the layout page are not checked by a tool in CI. The build of the layout page fails when a bundled package has no licence file, and `THIRD-PARTY.md` of the worker is kept by hand (ADR 0610).
- The Linux packages and the Windows package of each commit are built from debug builds of the client and checked; release builds and their publication are not part of this workflow. The macOS bundle of the product is checked by the manual release check of `packaging/check_release.py`.
- On GitHub, the Linux job builds the Linux image in every run; no cache keeps the image or the target directory between runs, so every run compiles the workspace. The runner of a private repository has two processors; the container runner starts one Cargo job for each processor of its set. On the Linux host the image and the volumes stay, and a run compiles what changed.
- A CI result on the project's machines is recorded on those machines only. A pull request shows no result; the person who merges reads `ci.sh status <commit>`.
- The images job on the macOS machine builds images for arm64; the images of the deployment for x86-64 are not built by CI while that job runs there.
- The Windows job does not run until the Windows virtual machine exists.
- The Linux image reads its packages from `snapshot.debian.org`. The job fails while that service cannot be reached.
- A new version of an action is a change of its commit hash in the workflow file.
- `cargo deny` is compiled in its job when the cache has no copy.
