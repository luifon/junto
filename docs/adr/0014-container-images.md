# 0014. Container images and how they are built

## Status

Accepted

## Context

The system is deployed with Docker Compose, with one container per component (ADR 0003). The server needed an image. The server runs `ffmpeg` and `ffprobe` to join and check the files of a recording (ADR 0606), and serves the recording layout page from a directory (ADR 0615). The recorder and the server write below the same directory (ADR 0604), and the file modes of the recorder are not a setting. A build that fetches a dependency by a name that can change gives another result on another day. The worker image already compiles its own FFmpeg under the LGPL, because the worker links it (ADR 0610). The project publishes image definitions and no built image (ADR 0610).

## Decision

The server image:

- `deploy/images/server/Dockerfile` builds the server image from the repository root. Its last stage is the image.
- `JUNTO_SERVER_IMAGE` and `JUNTO_INIT_IMAGE` optionally name local release images.
  Builds use distinct tags to avoid concurrent exports to one name. A restore requires
  both explicit references and never builds them (ADR 0005).
- The binary is built with `cargo build --locked --release` from `Cargo.lock`, on the official Rust image.
- The recording layout page is built in its own stage with `npm ci --ignore-scripts` from `package-lock.json`, and copied to `/usr/share/junto/recording-layout`. The image sets `JUNTO_RECORDER_LAYOUT_DIR` to it.
- `ffmpeg` and `ffprobe` are compiled in the image from the FFmpeg release and checksum that the worker definition pins. The build is LGPL, static, with every native decoder, reader and filter and no external library, and without the encoders and writers that the server does not call. The build fails when a GPL, version-3 or non-free option is on, and when a component that the server calls is absent.
- The final stage is the slim Debian image with no package added. It contains the binary, the two programs, the page and two scripts: the hand-over of transcription jobs and the script of `init`.
- The image has no shell tool for HTTP. Its health check is the command `junto-server health` (ADR 0012).
- The server runs as the user 1001, which is the user of the recorder image. Both then own the files below the directory of the recordings, whatever mode the recorder gives them.
- The image listens on `0.0.0.0:8080`, so the server does not start without `JUNTO_PUBLIC_ORIGINS` (ADR 0117).
- The migrations stay in the binary and run at each start (ADR 0008). The image adds no migration step.

Pinned inputs:

- Every base image, and the Dockerfile frontend, is named by tag and digest. This includes the frontend line of the worker definition, which named a tag only.
- The Debian packages of the FFmpeg build stage of the server image, and of the speech tooling of the worker tests (`workers/transcription/tests/speech/Dockerfile`), come from one dated snapshot of the Debian archive: the instruction that installs them removes the package sources of the base image and writes the two sources of the snapshot.
- The images that the deployment pulls (PostgreSQL, Redis, LiveKit, LiveKit Egress, the entry) are named by tag and digest in the Compose files.
- The link image installs four Alpine packages at stated versions.
- `rust-toolchain.toml` states the Rust version. The server image and CI use that version.
- `tools/check-pins.sh` fails when the Rust version, the FFmpeg release or its checksum differs between two files, when the LiveKit or PostgreSQL version of the deployment differs from the version that the tests use, when an external image or a CI action is named without a digest or a commit hash, when a Python package of the speech tooling of the worker tests has no version, when an instruction of the server definition or of the speech definition that names a package program or the package sources has another form than the one that removes the sources of the base image, writes the two sources of the snapshot and installs, when the snapshot is not an argument of the same build stage, when the two definitions name different snapshots, and when a workflow installs a package with `apt-get` without a version (ADR 0905). CI runs it.

Other images:

- The worker image is the existing definition of ADR 0610, unchanged except for the pinned frontend.
- The entry is the image `livekit/caddyl4`: Caddy with the layer-4 module, the build that the plan of step 0 names. The project builds no entry image.
- The link image of the two-host form is built from `deploy/images/link/`: Alpine with `wireguard-go`, the `wg` tool, `socat` and `iproute2`, the programs that step 0 ran.

## Consequences

- A build needs network access to the registries of Docker, Rust, Node.js and Python, to the FFmpeg release server, to the Debian snapshot archive and to the Alpine package mirror.
- The server image has 232 MB on arm64: 43 MB of it are the two FFmpeg programs and 18 MB the server binary.
- The server and the recorder are the same user on the volume of the recordings. A change of the user of the recorder image needs the same change of the build argument `JUNTO_UID`.
- The FFmpeg of the server image reads what its native components read. A recording format that needs an external library needs a change of the build.
- The compilers of the worker's build stages still come from the package archive of its base image at the time of the build (ADR 0610). The FFmpeg stage of the server image and the speech tooling of the worker tests use a dated snapshot.
- The Debian snapshot archive keeps every state of the archive. A build of the two definitions needs it to be reachable.
- Alpine keeps one version of a package per release. When a pinned package of the link image is replaced, its build fails until the version in the definition is updated.
- The `wg` tool is under GPL-2.0. The link image runs it as a separate program, and the project does not distribute the image.
- A new Rust version is a change of `rust-toolchain.toml` and of the tag and digest in the server definition, in one commit.
- The two scripts need bash, which the Debian image has. The scripts of the backup and of the restore run `bash`, `tar` and `find` of the image (ADR 0021).
