# Release tooling

The packages, the signed release format and the update step are described in ADR 0724 (format and source), ADR 0725 (macOS bundle and signing identity), ADR 0726 (archive, record and rollback), ADR 0754 (Linux) and ADR 0755 (Windows). The install instructions for people are `docs/client-macos.md`, `docs/client-linux.md` and `docs/client-windows.md`.

The end-to-end release procedure is in [docs/release.md](../docs/release.md). The build machine needs Python 3.11 or later.

## Keys

Create distinct release and freshness keys:

```sh
tools/serial.sh cargo run -p junto-client-core --bin junto-release -- \
    keygen /absolute/offline/release.seed /absolute/offline/release-public.json
tools/serial.sh cargo run -p junto-client-core --bin junto-release -- \
    keygen /absolute/freshness/freshness.seed /absolute/freshness/freshness-public.json
```

Each seed is generated randomly, written privately and never overwritten. Keep both outside repositories with encrypted offline backups. Copy only the release public array to `packaging/update-keys.json` and the freshness public array to `packaging/freshness-keys.json`, review and track both. A key in both files stops the build. Release keys authorize code and are brought online only for a new release. Keep freshness keys in a separate private signing environment for renewal before each 30-day expiry; a freshness key cannot authorize new code. The release host needs neither private key.

The release tool and every package are built with the same `JUNTO_UPDATE_SOURCE`, `https://github.com/luifon/junto/releases/`, from the same checkout. The tool uses the trust sets compiled into it.

## The release source

The source is the GitHub releases of the repository (ADR 0724). The release of version V has the tag `v<V>` and holds, for every target, the archive `junto-<V>-<system>-<package>-<architecture>.junto`, the release information `release-<V>-<system>-<package>-<architecture>.json` and `latest-<system>-<package>-<architecture>.json`, the release information of the newest release. A client reads the latest file at `https://github.com/luifon/junto/releases/latest/download/latest-<target>.json`, which GitHub serves from the newest release that is neither a draft nor a pre-release, and every file of version V at `https://github.com/luifon/junto/releases/download/v<V>/<file>`. Keep the files of earlier releases: an update downloads the installed version from them to keep it for a rollback. The release commands write into a new, empty directory for each release, because they never replace a file. The targets are `macos-bundle-aarch64`, `linux-directory-x86_64`, `linux-appimage-x86_64` and `windows-directory-x86_64`. The first-install files are in the same release.

## macOS

`bundle.py <binary> <new Junto.app> <recovery program> <icon program>` assembles both executables, metadata using the workspace version, and the icon file that the program `junto-icon` of the same build writes (ADR 0756). The fixed identifier is `org.junto.app` (ADR 0725).

Create the code-signing identity once, outside every repository:

```sh
python3 packaging/create_identity.py /absolute/private/code-signing
```

The new directory holds the certificate, private key, dedicated keychain and password. Keep it private on encrypted storage with an encrypted offline backup. Record the printed public fingerprint in `packaging/certificate.sha1` and track that file. Reuse the exact certificate for each release; a replacement changes permission identity. Signing and publication refuse an empty pin, another certificate or an ad-hoc identity. The certificate is valid for ten years.

```sh
JUNTO_CODESIGN_IDENTITY='<certificate fingerprint>' \
JUNTO_CODESIGN_KEYCHAIN='/absolute/private/code-signing/signing.keychain-db' \
    packaging/sign.sh /absolute/output/Junto.app
```

The helper signs the recovery program and then the bundle. It temporarily includes the dedicated keychain in the search list, restores the original list, and never changes default keychain or system trust. A fixed per-user lock serializes search-list changes independently of TMPDIR. Interrupt, termination and hangup are handled, including interruptions during restoration. Password commands use standard input; the working PEM and password files remain unencrypted secrets. Never put them in a shared folder, repository, release directory or unencrypted backup. SIGKILL and power loss cannot run cleanup: inspect the search list after either.

Create the macOS release files without uploading them:

```sh
JUNTO_UPDATE_SOURCE='https://github.com/luifon/junto/releases/' \
JUNTO_UPDATE_KEY='/absolute/offline/release.seed' \
JUNTO_FRESHNESS_KEY='/absolute/freshness/freshness.seed' \
JUNTO_CODESIGN_IDENTITY='<certificate fingerprint>' \
JUNTO_CODESIGN_KEYCHAIN='/absolute/private/code-signing/signing.keychain-db' \
    packaging/release.sh /absolute/new-release-directory
```

Use a new absolute output directory. The command builds the version, assembles and signs the bundle, checks its identity, compiled trust and version, and writes the archive, both release informations, the first-install ZIP, which holds `Junto.app` alone, and the disk image `Junto-<version>-macos-<architecture>.dmg` with `Junto.app` and a link to Applications, signed with the same identity (`dmg.py`, ADR 0728). No Python is needed on the installed machine. The internal `release_locked.sh` entry point requires its caller to hold the shared lock.

## Linux

On an x86-64 Linux host with Docker, build the packages in the Linux build image:

```sh
JUNTO_UPDATE_SOURCE='https://github.com/luifon/junto/releases/' \
    packaging/release_linux.sh target/linux-release
```

The output directory holds the tarball, the AppImage, the Debian package `junto_<version>_amd64.deb` (ADR 0728), and the source directories `package-directory` and `package-appimage` of the two update archives. `linux.py` takes the program `junto-icon` of the same build with `--icon-tool` and renders the icon theme with it (ADR 0756). `linux.py` stops when the client links a library that is neither in its bundled list nor in its host list (ADR 0754). Publish both with the release tool of the same checkout, on any system:

```sh
target/release/junto-release publish linux directory x86_64 target/linux-release/package-directory \
    /absolute/new-release-directory /absolute/offline/release.seed /absolute/freshness/freshness.seed
target/release/junto-release publish linux appimage x86_64 target/linux-release/package-appimage \
    /absolute/new-release-directory /absolute/offline/release.seed /absolute/freshness/freshness.seed
```

## Windows

On a Windows machine with the Rust toolchain of `rust-toolchain.toml` and the MSVC build tools, with `JUNTO_UPDATE_SOURCE` set:

```sh
CARGO_PROFILE_RELEASE_DEBUG=line-tables-only cargo build --locked --release -p junto-client --bin junto -p junto-client-core --bin junto-recover --bin junto-installer --bin junto-release
python packaging/windows.py target/release/junto.exe target/release/junto-recover.exe <version> <new output directory> \
    --installer target/release/junto-installer.exe --release-tool target/release/junto-release.exe
```

The output directory holds the ZIP, the setup program `Junto-<version>-windows-x86_64-setup.exe` (ADR 0728), the source directory `package-directory`, and `Junto-<version>-windows-x86_64.pdb`, the symbols of `junto.exe` with the line tables of every crate. The package does not carry them; publish the file with the release (ADR 0755, ADR 0757). `windows.py` refuses a build without symbols. The build script of the client puts the icon into `junto.exe` as its icon group 1 (ADR 0756). Publish it with the release tool of the same checkout, on any system:

```sh
target/release/junto-release publish windows directory x86_64 <output>/package-directory \
    /absolute/new-release-directory /absolute/offline/release.seed /absolute/freshness/freshness.seed
```

## Freshness

Renew the freshness of the newest release at most 21 days after its last statement, even without a new release, on the machine that holds the freshness key:

```sh
python3 packaging/renew_freshness.py /private/releases/0.1.0/junto-release /private/releases/0.1.0/assets \
    /private/freshness/freshness.seed --upload v0.1.0
```

The first argument is the release tool built with the release, the second the retained output directory of `release_assets.py`. For each target the command runs the release tool:

```sh
junto-release refresh <directory>/latest-<target>.json <directory>/junto-<version>-<target>.junto <version> /private/freshness/freshness.seed
```

The tool verifies release signatures, version, archive size, hash and signature with its compiled trust sets before atomically replacing only freshness information. It never needs a release private key. The command requires 29 days or more of validity afterwards. With `--upload <tag>` it first requires the tag to be `v<version>` of the directory and the GitHub command to be signed in, then replaces the four latest files of that GitHub release with `gh release upload --clobber` (the GitHub command-line tool, signed in with the right to publish releases) and reads each one back from `https://github.com/luifon/junto/releases/latest/download/`. Never refresh files fetched back from GitHub. A missed date or incorrect client clock prevents updating until freshness is valid again. A client refuses a statement that expires more than 45 days after its clock.

For release-key rotation, build clients with both public keys and pass additional release-secret paths after the freshness secret of `publish` or `release.sh`; both keys then sign the release and archive. The lower-level `package SOURCE OUTPUT RELEASE-SECRET FRESHNESS-SECRET VERSION SYSTEM PACKAGE ARCH [RELEASE-SECRET...]` supports the same overlap. For freshness-key rotation, include both freshness public keys and pass both freshness secrets to `refresh` before publishing. Keep signatures for every supported old trust set; an overlap client alone does not reach users who skipped that version.

## Licences

Each package holds `licences/Junto-LICENSE.txt` and `licences/third-party-licences.txt` (in the macOS bundle under `Contents/Resources`). `licences.py` writes the second file during the assembly of the package: every crate of the programs `junto` and `junto-recover` for the target of the package, with its licence and notice texts, the WebRTC library, the Rust standard library, the fonts, on Linux the bundled libraries, and in the AppImage the components of its runtime, whose source the AppImage holds in `licences/appimage-runtime` (ADR 0801). The line of a crate states when its licence files do not hold the text of its declared licence, or hold the text of a licence that it does not declare. It runs `cargo tree` and `cargo metadata` in the checkout and reads the licence file of the WebRTC library from the build directory of the programs, so assemble a package from the checkout and the build that made its programs. `python3 packaging/licences.py client <system> <architecture> <build directory> <new directory>` writes the two files alone; with `server`, the files of the server image, which its Dockerfile writes. A crate whose licence files do not hold the text of its declared licence, and whose licence has no text in `licence-texts/`, stops the assembly; add the text there (`licence-texts/README.md`).

## Test configurations and checks

External test-only public-key, freshness-key, certificate and version variables mark the build as "test configuration" in Version and `--version`. `publish` of such a build requires `--test-configuration`; a product build refuses that flag for every command. The public release wrappers refuse overrides. Tests never rewrite tracked trust or version files. `JUNTO_TEST_KEYCHAIN_LOCK` selects an isolated lock for controlled tests; every unit test sets it.

CI runs the portable Rust release, installer and configuration tests and the Python unit tests on every system. `check_signing.py` compares certificate requirements using paths with spaces and checks keychain restoration. `check_update.py` builds the release tool and the recovery program with test keys and a test certificate, then runs a real update of signed test bundles from a local source, which `release_tools.py` serves in the layout of GitHub releases with their two redirects, and the documented recovery command while the new executable fails; these fixture executables only end with a status. It then runs `check_lifecycle.py` on the real client: two builds of the checkout with temporary test keys, the workspace version and the next patch version, in the target directory `lifecycle` inside the target directory. The first package is installed in a folder whose name holds a space and a character outside ASCII; the window of the installed client finds the newer version on a local source, installs it with the update action of its version line and offers the restart (`--update-source` and `--check-update`, which only a test configuration accepts); the recovery program rolls the update back. At each of the three stages the installation must equal the package of the expected version file by file (on macOS also `codesign --verify --deep --strict`), and the client must state that version and draw its first frame with a new empty profile. `check_linux.py` (in the Linux build image) and `check_windows.py` check the Linux and Windows packages (ADR 0754, ADR 0755) and run the same lifecycle; Linux starts under X11 and Wayland and uses FUSE for the AppImage when its prerequisites are there, else states why it uses extract-and-run. The processes of every command are ended when it returns. These checks build in the repository's target directory; a local caller holds `tools/serial.sh`.

`taskbar_clicks.py <folder>\junto.exe` runs on Windows in the desktop session of a person, not in CI: it starts the program with an empty temporary data directory, presses its taskbar button three times through the input of the system, and prints after each press whether the main window is minimized, which window is in front and how many taskbar buttons name Junto, and then which explanation of the first press that does not restore the window the record fits (ADR 0755). It moves the pointer and ends the program afterwards.

`check_release.py`, under the shared lock, builds two real macOS product releases with temporary role-separated keys. It checks test-build marking, publication refusal, compiled-source installation and the documented recovery command while the new executable fails. It removes temporary identities and files; remove the release build output afterwards. `check_start.py <Junto.app>` is separate: it starts the bundled executable with an empty temporary data directory, `--no-activate` and `--check-startup`, and requires the first-frame line. Neither packaging nor signing grants microphone, camera, screen or notification permission.

## The GitHub release

`release_assets.py` collects the files of one release, or of one test build, for its GitHub release (ADR 0727). It needs Python 3.11 or later and no build:

```sh
python3 packaging/release_assets.py 0.1.0 /private/releases/0.1.0/all /private/releases/0.1.0/assets
```

- The second argument is a directory with every file of the release from every system: the first-install files `Junto-<version>-macos-aarch64.dmg`, `Junto-<version>-macos-aarch64.zip`, `Junto-<version>-windows-x86_64-setup.exe`, `Junto-<version>-windows-x86_64.zip`, `Junto-<version>-x86_64.AppImage`, `junto-<version>-linux-x86_64.tar.gz` and `junto_<version>_amd64.deb`, the symbols file `Junto-<version>-windows-x86_64.pdb`, and the latest file, release file and archive of each of the four targets. A missing or unknown file, or update files of one target that do not belong together, stop the command; the error names each file.
- The third argument is the new output directory. It must not exist; a failure leaves no output directory. The command does not change the release directory.
- A release has the tag `v<version>` and the title `Junto <version>`. `--notes` sets the notes file (default `docs/releases/<version>.md`).
- `--test <n>` collects test build `n` of the version: the first-install files and the symbols file only. The update files of the version may be in the release directory; they are left out without a check. The tag is the lightweight tag `v<version>-test.<n>` on the release commit, the title `Junto <version> test <n>`, and `--notes` is required. A test build is never edited into a release; the release of the version is collected again without `--test` and published as a new GitHub release of the tag `v<version>`.

```sh
python3 packaging/release_assets.py 0.1.0 /private/releases/0.1.0/all /private/releases/0.1.0/test-4 \
    --test 4 --notes /private/release-notes/v0.1.0-test.4.md
```

The output holds every collected file and `SHA256SUMS`, which lists every file except the latest files. The command prints every file with its size and the publication: for a release, `gh release create v<version>` of a draft with every file and the notes, then `gh release edit --draft=false --latest`; for a test build, `git tag v<version>-test.<n> <release commit>` (push the tag before the draft), `gh release create v<version>-test.<n>` of a draft, then `gh release edit --draft=false --prerelease`. Keep the output directory of a release: the freshness renewal reads it. A test build has no latest files and no freshness renewal.
