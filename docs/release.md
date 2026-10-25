# Release Junto

A release names one reviewed commit, one workspace version and its packages and server
images. The commands below prepare files; the operator supplies the release host, registry
and private signing environment. The repository does not currently publish releases
through a tag or scheduled job.

## Before the first release

Choose a stable HTTPS release directory and an image registry. Clients compile that
HTTPS address as `JUNTO_UPDATE_SOURCE`; changing it later does not redirect clients that
are already installed. Keep earlier archives at that address for recovery.

Follow [the key and certificate instructions](../packaging/README.md#keys) to create
separate release and freshness keys and the permanent macOS signing identity. Track only
`packaging/update-keys.json`, `packaging/freshness-keys.json` and
`packaging/certificate.sha1`, then review and commit them before building. Empty trust
files do not make an installable update source. Keep the private material outside the
checkout and the release host, with encrypted backups.

Arrange renewal of freshness information before its 30-day expiry. The renewal signer
needs the retained, verified release files and the freshness key; it needs no release key.
The repository supplies the renewal command, but installs no scheduled job or hosting
credentials. [Packaging](../packaging/README.md#freshness) describes rotation and renewal.

## Choose and verify the commit

1. Change the workspace version in `Cargo.toml` and update its lock file if necessary.
   Keep the server, clients, release tool and recovery program on the same commit.
2. Run the checks of [AGENTS.md](../AGENTS.md#before-a-commit), then commit. Run CI on that
   exact commit on Linux, macOS and Windows. `tools/ci/ci.sh status <commit>` must report
   `passed`; a job on another commit is not evidence for this release.
3. Run the real-system checks of [ADR 0902](adr/0902-testing-strategy.md), including
   packaged installation, update and recovery, and the device checks on the supported
   systems. CI uses synthetic media and does not establish real capture or permissions.
4. Retain the commit, version, CI run identifiers, checks and package hashes with the
   release record. Create the release tag only after these checks. Do not reuse a version
   whose files were already published.

## Build the client packages

Use a clean checkout of the chosen commit on each build system, the toolchain pinned in
that checkout, the same HTTPS source, and the tracked trust files. No test-configuration
variable may be set. Run heavy commands through the machine lock; the Linux release
wrapper takes that lock itself.

- On macOS, `packaging/release.sh` builds and signs the bundle and prepares its first-install
  ZIP and signed update files. Follow [the macOS commands](../packaging/README.md#macos).
- On x86-64 Linux, `packaging/release_linux.sh` prepares the tarball, AppImage and their
  package directories. Use `junto-release publish` for both package kinds, as described
  in [the Linux commands](../packaging/README.md#linux).
- On x86-64 Windows, build with line tables and assemble with `packaging/windows.py`, then
  use `junto-release publish` for the directory package. The output also holds
  `Junto-<version>-windows-x86_64.pdb`, the symbols of the program, which the package does
  not carry. Publish it as a file of the release with the ZIP, in the checksums: a panic
  report of a Windows client is resolved with it (ADR 0757). Follow
  [the Windows commands](../packaging/README.md#windows).

Use a new output directory for each publication command. Keep the release tool built
from this commit with the same source and trust sets. Check the version and test marker
of every binary; retain the licence files included by the package builders. Run the
package checks on each system before copying the files to the release host.

## Build and retain the server images

From the same checkout, build the server and init images and the worker variant used by
the deployment. These examples use a local version tag; replace `VERSION` with the chosen
workspace version:

```sh
tools/serial.sh docker build -f deploy/images/server/Dockerfile -t junto-server:VERSION .
docker tag junto-server:VERSION junto-init:VERSION
tools/serial.sh docker build -f workers/transcription/docker/Dockerfile --build-arg VARIANT=cpu \
    -t junto-worker:VERSION workers/transcription
```

The init service uses the same image with a different entry point; it has no separate build stage.
For a GPU deployment, build the worker with `VARIANT=gpu` and retain it as a distinct
image. Run the deployment tests described in [deploy/README.md](../deploy/README.md) and
verify the model manifest using the worker of this release. Record each published image
by its registry digest, including server and init; a mutable tag alone cannot identify
the tested image. The deployment builds the worker from its pinned Dockerfile and keeps
the external image digests in its Compose files. Retain the checkout, image archives or
registry digests, and models needed to reproduce or restore that deployment.

## Publish and check

Upload immutable versioned archives, metadata and first-install packages first. Verify
that they can be fetched from the chosen HTTPS source. Replace each target's `latest-…json`
only after its referenced archive is present, using an atomic replacement supported by
the host. Retain the previous versioned files; overwriting a latest pointer does not
replace those files. Do not upload a private key, keychain, signing password or test key.

Test a first install and an update from the preceding supported release through the
public source on every system. Check signature verification, the displayed version,
first frame and recovery to the previous version. Publish the release notes, supported
targets, checksums and server image digests with the release tag. Hosting, upload access,
registry publication and the recurring freshness job require operator configuration;
the package commands alone do not publish a release.

## Upgrade a server or withdraw a release

Follow [installation, backup and upgrade](install.md) on the deployment. Take and verify
a backup, stop recordings through the client and wait for completion before stopping
services. Preserve the previous images and data required by the documented undo path.
The server retains a pre-migration database copy, but that copy does not replace a backup
of files, settings and keys.

If a client release is faulty, stop offering it as the latest release, keep its immutable
files for recovery, and publish a corrected release with a higher version. Test the
recovery commands documented for [macOS](client-macos.md), [Linux](client-linux.md) and
[Windows](client-windows.md). Restore a server only through the procedure for its backup
and schema version; replacing its executable alone does not undo a database migration.
