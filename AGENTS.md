# Guide to the repository

This file is for a person or a coding agent who opens the repository. It states where the parts of Junto are, how to build and test them, and the rules for a change. Each statement names the file that holds the details. When this file and the named file differ, the named file is correct, and this file must be corrected (ADR 0901).

## What Junto is

Junto is a self-hosted application for team text chat, voice and video calls, screen sharing, and call recording with transcripts. It has a server in Rust with PostgreSQL, a native desktop client built with gpui for macOS, Windows and Linux, LiveKit as the media server, a recorder, and a transcription worker.

- `docs/scope.md`: what the first version contains, what later versions can add, and what is not planned.
- `README.md`: the main design points.

## Crates

The Rust workspace is `Cargo.toml` at the root, with the members `crates/*`. The Rust version is in `rust-toolchain.toml`.

| Crate | Path | Content |
|---|---|---|
| `junto-protocol` | `crates/protocol` | The request, response and event types of the public API. The server and the client use them. |
| `junto-authz` | `crates/authz` | The permission model and the one function, `authorize`, that decides every permission of the server and of the client. |
| `junto-server` | `crates/server` | The server, with its migrations in `crates/server/migrations`. The binary `junto-server`, and `junto-openapi`, which writes the API documents in `docs/api/`. `crates/server/README.md` explains how to run it, its settings and its tests. |
| `junto-setup` | `crates/setup` | The setup tool: writes the structure of a server to a file and applies a file to a server, through the public API with an access token. |
| `junto-client-core` | `crates/client-core` | The client without a user interface: local storage, device keys, the API client, and the client flows. The binaries `junto-cli` (operates servers from a terminal), `junto-release` (creates releases) and `junto-recover` (the recovery program of an installed package). |
| `junto-media` | `crates/media` | The media engine of the client: sound devices, the media connection of a call, and the one call of a client over all its servers. See `crates/media/README.md`. |
| `junto-client` | `crates/client` | The desktop application: a gpui window on the client core. The binary `junto`, and `junto-screens`, which draws every screen to a PNG file without a window. `crates/client/README.md` has the layout of the crate. |
| `junto-icon` | `crates/icon` | Renders `packaging/icon/junto.svg` into the icon files of the packages. |

Outside the workspace:

- `workers/transcription/`: the transcription worker, in Python (ADR 0610 to 0614).
- `recording-layout/`: the browser page that the recorder draws for the composite recording (ADR 0602, ADR 0609).
- `spikes/`: the technical tests of step 0. They are not product code. `spikes/test-participant` is the call participant without a user interface that the server live tier and the deployment tests use.

## Decisions: `docs/adr/`

- `docs/adr/README.md` is the index. Numbers are grouped in ranges by area; a new record takes the next free number in the range of its area.
- ADR 0901 states how the repository is kept:
  - No personal information in a tracked file: no names, host names, addresses or account identifiers. Examples and test data use invented values.
  - Comments are kept to a minimum.
  - An ADR states engineering decisions and their technical reasons only.
  - Every feature adds or changes an ADR, so that `docs/adr/` describes the current design.
  - The order for each feature: tests and a check in the running application, an independent review, fixes, the ADR, a local commit, and a push after a review by a maintainer.
- ADR 0902 is the testing strategy. ADR 0904 covers the server tests, ADR 0905 continuous integration, ADR 0906 the bound on the stack of one function, ADR 0912 the window tests on Windows, ADR 0913 the native media tests, ADR 0914 the measurement of the client per mode.
- `docs/step-0/README.md` lists the technical tests of step 0 (ADR 0903).

## Documents for people who install Junto

- `docs/install.md`: install a server with Docker Compose (one host, two hosts, a trial on one machine), and back it up, restore it, upgrade it and remove it.
- `docs/client-macos.md`, `docs/client-linux.md`, `docs/client-windows.md`: install the desktop client from its package, how it updates, and how to restore the previous version.
- `docs/api/`: `openapi.json`, `events.json` and `structure-file.json`, generated from the code. After a change of a route, a type or an event, run `cargo run -p junto-server --bin junto-openapi` and commit the files. The test `openapi` of `junto-server` fails while a committed file differs from the code.

## Release

The release document does not exist yet. When it is written, it is linked in this section.

Until then: `packaging/README.md` describes the release tooling, the keys and the release source. ADR 0724, 0725, 0726, 0754 and 0755 describe the release format and the packages, and ADR 0902 names the systems that a release needs.

## `deploy/`

The files that install a server with Docker Compose. `deploy/README.md` lists each file; the decisions are in ADR 0003, 0004, 0014, 0015, 0016, 0018, 0021 and 0022.

- Compose files: `base.yaml` (each service, not used alone), `one-host.yaml`, `two-hosts-data.yaml` with `two-hosts-public.yaml`, `local.yaml` (one machine without DNS names and TLS, for a trial and for the tests), `worker-gpu.yaml`, `restore-images.yaml`.
- `env.example`: every setting. A deployment copies it to `.env`, which is not tracked.
- Scripts: `generate-secrets.sh`, `check-limits.sh`, `backup.sh`, `restore.sh`, `upgrade-undo.sh`, `database-user.sh`.
- `images/`: the images of the server, the worker, the recorder, the link and PostgreSQL. `entry/Caddyfile`: the entry on port 443.
- `tests/`: the deployment tests. They need Docker; the commands are in `deploy/README.md`.

## `packaging/`

The release tooling of the client: the macOS bundle and its signing, the Linux tarball and AppImage, the Windows ZIP, the update and recovery checks, and the public release and freshness keys (`update-keys.json`, `freshness-keys.json`). `packaging/icon/` holds the source of the application icon. The unit tests are the files `test_*.py`; the files `check_*.py` are checks that build and run packages. See `packaging/README.md`.

## `tools/`

- `tools/serial.sh <command>`: runs one heavy command at a time on the machine, with a shared lock file, at a lower priority. Run `cargo build`, `cargo test`, `cargo clippy` and Docker runs through it when other work shares the machine. `JUNTO_HEAVY_LOCK` names another lock file.
- `tools/with-test-postgres.sh <command>`: starts a temporary PostgreSQL in Docker, gives the command `JUNTO_TEST_DATABASE_URL`, and removes the container with its volume. After a failure it prints the container state and its logs. `JUNTO_TEST_POSTGRES_CONTAINER` names the container (default `junto-dev-postgres`).
- `tools/with-test-livekit.sh <command>`: starts a temporary LiveKit server in development mode and removes it. It gives the command `JUNTO_TEST_LIVEKIT_API_URL`, `JUNTO_TEST_LIVEKIT_URL`, `JUNTO_TEST_LIVEKIT_API_KEY`, `JUNTO_TEST_LIVEKIT_API_SECRET`, `JUNTO_TEST_WEBHOOK_ADDRESS` and `JUNTO_TEST_LIVEKIT_CONTAINER`. Its settings: `JUNTO_TEST_LIVEKIT_CONTAINER` (default `junto-dev-livekit`), `JUNTO_TEST_LIVEKIT_PORT`, `JUNTO_TEST_LIVEKIT_TCP_PORT` and `JUNTO_TEST_LIVEKIT_UDP_PORT` (defaults 17880, 17881, 17882), `JUNTO_TEST_WEBHOOK_ADDRESS` (default `127.0.0.1:17890`; on Linux an address of the machine that the container reaches), `JUNTO_TEST_LIVEKIT_CPUS`, `JUNTO_TEST_LIVEKIT_MEMORY`, and `JUNTO_TEST_LIVEKIT_DEPLOYMENT_LAYERS=1`, which gives the camera layers the setting of the deployment (the layout tests set it, ADR 0609).
- Two checkouts that run tests on one machine at the same time each set their own value of these six variables: `JUNTO_TEST_POSTGRES_CONTAINER`, `JUNTO_TEST_LIVEKIT_CONTAINER`, `JUNTO_TEST_LIVEKIT_PORT`, `JUNTO_TEST_LIVEKIT_TCP_PORT`, `JUNTO_TEST_LIVEKIT_UDP_PORT` and `JUNTO_TEST_WEBHOOK_ADDRESS`.
- `tools/linux-container.sh <command>`: runs a command in the Linux build image (`tools/linux/Dockerfile`, ADR 0750), with its own PostgreSQL, LiveKit and PipeWire. It takes the lock of `tools/serial.sh` itself; do not wrap it in `tools/serial.sh`.
- `tools/dev-server.sh`: a development server with a temporary database; it prints the address and the one-time owner code. `--media` also starts LiveKit.
- `tools/check-pins.sh`: fails when a version that two files state differs between them, or when an image, a CI action, a Debian package of an image or a package that a workflow installs is not pinned (ADR 0014, ADR 0905).
- `tools/process_usage.py <path>`: reads the memory, the processor time and the threads of every process whose executable is under a path, by the method of the `modes` target, to compare another application with the client (ADR 0914).
- `tools/check-condition-wait.sh` (ADR 0786), `tools/check-client-start.sh` (a native start of the client with an empty data directory), `tools/native-devices.sh` (ADR 0913).
- `tools/linux/`: the Linux build image and its runner. `tools/windows/`: the setup and the lock of the Windows CI machine, and PostgreSQL for the window tests on Windows (ADR 0912).

## Continuous integration: `tools/ci/` (ADR 0905)

- Each job is one script: `rust-linux.sh`, `rust-other-systems.sh` (macOS and Windows), `licences.sh`, `layout-page.sh`, `transcription-worker.sh`, `deployment.sh`. The images and tool versions of the jobs are in `tools/ci/pins.sh`.
- `.github/workflows/ci.yml` calls the same scripts. Each action is named by its commit hash.
- CI runs on three machines of the project and is started with `tools/ci/ci.sh`:
  - `ci.sh run <commit>` sends the commit to the Linux host and to the Windows machine over SSH and starts their jobs: on Linux `rust-linux`, `licences`, `transcription-worker`, `layout-page`; on Windows `rust-windows`.
  - `ci.sh local <commit>` runs the macOS jobs on the local machine: `rust-macos` and `deployment`.
  - `ci.sh status <commit>` prints `passed` when the latest run on each of the three machines finished with every job passed.
  - The addresses of the machines come from `.env.local`, which is not tracked.
- `run-jobs.sh` runs the jobs of a run on one machine, each in a group of processes of its own (`group.py`), with the run lock of `tools/serial.sh --inherited`.
- `caches.py` keeps the caches of each machine under their size caps before and after each run, empties a target cache that another runner version wrote, and before a run decides from the free space and the recorded use of earlier runs whether the run keeps them, empties them or starts no job; a machine sets other limits in the file `settings` of its CI directory.
- The Linux job runs format, clippy, the workspace build and suite, the Python unit tests, the check of the Linux packages, and the comparison of `docs/api/` with the generated files, in the Linux build image. The macOS and Windows jobs run clippy, build every target and run the tests that need no database; macOS also runs the media unit tests and the signing and update checks; Windows also runs the check of the Windows package and the window tests with PostgreSQL.

## `vendor/`

Copies of the build inputs of `livekit` 0.9.4, `webrtc-sys` 0.3.48 and `webrtc-sys-build` 0.3.19, with corrections. The `[patch.crates-io]` section of `Cargo.toml` and `spikes/test-participant` use them. Each change carries a `Modified:` comment.

- `vendor/README.md`: the checksum of each source archive, the files that were kept, each correction, when to remove it, and the tests to run when a copy is replaced.
- ADR 0733 explains each correction and its regression checks. A change of a copy also changes `vendor/README.md` and ADR 0733.

## Build

```sh
cargo build --workspace
```

- On Windows every crate uses the static C runtime (`.cargo/config.toml`, ADR 0753).
- On Linux the system packages are those of `tools/linux/Dockerfile`; `tools/linux-container.sh cargo build --workspace` builds in that image.
- The workspace lints are in `Cargo.toml`: unsafe code is forbidden (`junto-media` and `junto-client-core` allow it in named modules), `unwrap` and `expect` are denied, and the stack of one function has the bound of `clippy.toml` (ADR 0906).
- A local server and client: `crates/client/README.md`, section "Run it locally".

## Test tiers

Run each heavy command through `tools/serial.sh` when other work shares the machine. The tests of `junto-client`, `junto-client-core` and `junto-media` that need a server start the `junto-server` binary of the target directory (or `JUNTO_SERVER_BIN`), so run `cargo build --workspace` first.

- **Workspace suite.** Needs Docker, and `ffmpeg` and `ffprobe` with `libopus` and `libx264`. It does not run ignored tests.

  ```sh
  cargo build --workspace
  tools/with-test-postgres.sh tools/serial.sh cargo test --workspace
  ```

  The tests that need no database: `cargo test -p junto-authz -p junto-protocol`.

- **Window live tier.** The window against a real server and LiveKit:

  ```sh
  tools/with-test-postgres.sh tools/with-test-livekit.sh tools/serial.sh \
      cargo test -p junto-client --test app live:: -- --ignored
  ```

- **Media live tier.** The media engine against a real server and LiveKit; `--test calls` limits it to the call tests:

  ```sh
  tools/with-test-postgres.sh tools/with-test-livekit.sh tools/serial.sh \
      cargo test -p junto-media -- --ignored
  ```

  On Linux: `tools/linux-container.sh cargo test -p junto-media --no-fail-fast -- --ignored`. It includes the TURN TLS trust test and the tests with virtual sound devices, which need `JUNTO_TEST_VIRTUAL_AUDIO=1`; the container sets it. Do not set it on a desktop session.

- **Server live tier.** The server against LiveKit with the participants of `spikes/test-participant`, and a tier with the real transcription worker (`transcriber_real::`). The commands are in `crates/server/README.md`, section "Tests".

- **Native targets on macOS** (ADR 0902). They run without the test runner, on the main thread, and do nothing without their variable:
  - `frames` (ADR 0774): `JUNTO_TEST_FRAMES=1 tools/with-test-postgres.sh tools/with-test-livekit.sh tools/serial.sh cargo test -p junto-client --test frames`
  - `endurance`: `JUNTO_TEST_ENDURANCE=1` with the same wrappers and `--test endurance`. `JUNTO_TEST_ENDURANCE_MINUTES` sets the measured time (default 10).
  - `modes` (ADR 0914): `JUNTO_TEST_MODES=1` with the same wrappers, `--release` and `--test modes`, after `cargo build --release -p junto-server` and the release build of `spikes/test-participant`. It measures one window per process in six modes and prints the table; `JUNTO_TEST_MODES_PROFILE` names a directory for the profiles of each mode. `docs/client-performance.md` has the measured figures.
  - `rehearsal`: against a deployment installed with `docs/install.md` ("Trial on one machine"), with `JUNTO_REHEARSAL_ADDRESS`, `JUNTO_REHEARSAL_OWNER_CODE_FILE`, `JUNTO_REHEARSAL_SPEECH` and `JUNTO_REHEARSAL_WORK`. See `crates/client/tests/rehearsal/main.rs`.
  - Native device stages (ADR 0913): `JUNTO_TEST_NATIVE_DEVICES=1 tools/native-devices.sh <stage>`. They open real devices and need a person at the machine and the BlackHole virtual sound device; a permission request also needs `JUNTO_TEST_PERMISSION_REQUEST`. The stages that start services need the six variables above. See `crates/media/tests/native/README.md`.

- **Other tests.** The Python unit tests of `packaging/`, `tools/ci/`, `tools/linux/` and `tools/windows/` (`python3 -m unittest discover -s <directory> -p "test_*.py"`), `python3 tools/test_test_services.py` and `python3 tools/test_process_usage.py`; the deployment tests in `deploy/README.md`; the transcription worker and the layout page through their CI scripts in `tools/ci/`.

## Before a commit

These must pass:

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo deny check` (`deny.toml`, ADR 0801)
- `tools/check-pins.sh`
- the workspace suite
- the window live tier and the media live tier, when call, media or window code changed; the tests that `vendor/README.md` names, when a copy in `vendor/` changed
- the packaging unit tests, after `cargo build --workspace`, when `packaging/` or the release code of `junto-client-core` changed
- the regenerated `docs/api/`, when a route, a type or an event changed

Also:

- An ADR is added or changed for each decision (ADR 0901).
- The staged change contains no personal information, host names, addresses, account identifiers, keys or other secrets (ADR 0901).
