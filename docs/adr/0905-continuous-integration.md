# 0905. Continuous integration workflow

## Status

Accepted

Open: the workflow was checked with a linter for workflow files and was never run on a runner. The tests that need PostgreSQL do not run on macOS and Windows.

## Context

ADR 0902 decides that CI builds and tests on macOS, Windows and Linux for every commit, regenerates the API documentation and checks the licence of every dependency (ADR 0801, ADR 0202). The repository has a Rust workspace, the recording layout page with browser tests (ADR 0609), the transcription worker in Python (ADR 0610), and the container images and Compose files (ADR 0014, ADR 0015). The integration tests need a PostgreSQL that the test script starts in Docker (ADR 0904); the runners for macOS and Windows have no Docker for Linux containers. A workflow that names an action by a tag runs other code when the tag moves.

## Decision

- One workflow, `.github/workflows/ci.yml`, runs for every push and for every pull request from another repository.
- Every action is named by its commit hash, with the version as a comment. Every container image that a step runs is named by its digest: the workflow gives the two test scripts the PostgreSQL image and the LiveKit image of the deployment by digest. `tools/check-pins.sh` fails when one is not, and when those two differ from the images of the deployment.
- A package that a step installs with `apt-get` is named with its version: `ffmpeg`, which the Rust tests run to generate and read test media. `tools/check-pins.sh` fails for an `apt-get install` without a version in a workflow. Not pinned: the programs of the runner image, the packages that `ffmpeg` depends on outside its own source package, and the system packages that Playwright installs for its browser.
- The workflow has no permission by default. Each job gets `contents: read`. No job uses a secret, and the checkout keeps no credential.
- The runner images are named by version, not by `latest`.
- The Rust version is the one of `rust-toolchain.toml` (ADR 0014).

Jobs:

- Rust on Linux: `cargo fmt --check`, `cargo clippy` with warnings as errors, the tests of the workspace with `tools/with-test-postgres.sh` and `ffmpeg`, and the generation of the API documents followed by a comparison with the committed files.
- Rust on macOS and on Windows: a build of every target of the workspace, and the tests of `junto-authz`, `junto-protocol` and the library and tools of `junto-client-core`, which need no database.
- Licences: `cargo deny check` with a stated version of the tool.
- Recording layout page: the locked packages without install scripts, the build, and the browser tests against a LiveKit server in Docker.
- Transcription worker: the locked packages with `uv`, `ruff`, `mypy`, and the unit tests.
- Images and deployment: the check of pinned versions, the linter for workflow files and the linter for shell scripts from pinned images, the rendered Compose files of every form, the builds of the server image and the link image, the test of the hand-over scripts, the tests of the recorder start script and of the entry, the start of the local form, which builds the worker image, a backup of a running server with a restore into an empty deployment (ADR 0902, ADR 0911), an upgrade from an earlier release with the way back (ADR 0911), and the start of the one-host form and of the two-host form on the runner. This job fetches the whole history, because the upgrade test reads the files of an earlier commit.

- `tools/serial.sh` uses `flock` where `lockf` does not exist, so that the scripts that call it run on Linux.

## Consequences

- An error that exists only with PostgreSQL on macOS or Windows is not found by CI. ADR 0904 names a PostgreSQL for those runners as open.
- The tier of the call tests against a real LiveKit server, the tier with the real worker and its models, and the mutation tests of the layout page do not run in CI. They need a build of the test participant, the model files, and more time than a commit check has.
- The recording test of the deployment (`deploy/tests/recording.sh`, ADR 0910) does not run in CI. The storage guard starts a recording only with 13 GiB of free disk space (ADR 0605), and a hosted runner states 14 GB of disk, of which the images of the deployment take 5.3 GB without the cache of their builds. The recorder needs up to 3 CPU cores for the composite while the media server, the server and three test participants run, and a hosted runner has 4; a recorder that is short of CPU writes a file with holes (ADR 0602), so the test would fail without a defect. CI lints the scripts of the test.
- The licences of the Python packages and of the packages of the layout page are not checked by a tool in CI. The build of the layout page fails when a bundled package has no licence file, and `THIRD-PARTY.md` of the worker is kept by hand (ADR 0610).
- The packaged desktop client and its three systems (ADR 0704, ADR 0705) are not built by this workflow. A job for them is added with the client crate, and the Linux job then needs the system libraries of gpui.
- A new version of an action is a change of its commit hash in the workflow file.
- The step that installs `ffmpeg` fails when the package archive of the runner no longer has the named version. The version is the one of the release of the runner's system, which its archive keeps.
- `cargo deny` is compiled in its job when the cache has no copy.
