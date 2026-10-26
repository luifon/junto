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

The release tool and every package are built with the same `JUNTO_UPDATE_SOURCE` (the HTTPS base address of releases) from the same checkout. The tool uses the trust sets compiled into it.

## The release source

For every release and every target the source holds the archive `junto-<version>-<system>-<package>-<architecture>.junto` and the release information `release-<version>-<system>-<package>-<architecture>.json`; `latest-<system>-<package>-<architecture>.json` is the release information of the newest release. Keep the files of earlier releases: an update downloads the installed version from them to keep it for a rollback. The release commands write into a new, empty directory for each release, because they never replace a file; copy the files of that directory to the source beside the files of earlier releases. The targets are `macos-bundle-aarch64`, `linux-directory-x86_64`, `linux-appimage-x86_64` and `windows-directory-x86_64`. The first-install files (the macOS ZIP, the Linux tarball and AppImage, the Windows ZIP) are offered beside them.

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
JUNTO_UPDATE_SOURCE='https://<your-release-host>/<release-directory>/' \
JUNTO_UPDATE_KEY='/absolute/offline/release.seed' \
JUNTO_FRESHNESS_KEY='/absolute/freshness/freshness.seed' \
JUNTO_CODESIGN_IDENTITY='<certificate fingerprint>' \
JUNTO_CODESIGN_KEYCHAIN='/absolute/private/code-signing/signing.keychain-db' \
    packaging/release.sh /absolute/new-release-directory
```

Use a new absolute output directory. The command builds the version, assembles and signs the bundle, checks its identity, compiled trust and version, and writes the archive, both release informations and the first-install ZIP, which holds `Junto.app` alone. No Python is needed on the installed machine. The internal `release_locked.sh` entry point requires its caller to hold the shared lock.

## Linux

On an x86-64 Linux host with Docker, build the packages in the Linux build image:

```sh
JUNTO_UPDATE_SOURCE='https://<your-release-host>/<release-directory>/' \
    packaging/release_linux.sh target/linux-release
```

The output directory holds the tarball, the AppImage, and the source directories `package-directory` and `package-appimage` of the two update archives. `linux.py` takes the program `junto-icon` of the same build with `--icon-tool` and renders the icon theme with it (ADR 0756). `linux.py` stops when the client links a library that is neither in its bundled list nor in its host list (ADR 0754). Publish both with the release tool of the same checkout, on any system:

```sh
target/release/junto-release publish linux directory x86_64 target/linux-release/package-directory \
    /absolute/new-release-directory /absolute/offline/release.seed /absolute/freshness/freshness.seed
target/release/junto-release publish linux appimage x86_64 target/linux-release/package-appimage \
    /absolute/new-release-directory /absolute/offline/release.seed /absolute/freshness/freshness.seed
```

## Windows

On a Windows machine with the Rust toolchain of `rust-toolchain.toml` and the MSVC build tools, with `JUNTO_UPDATE_SOURCE` set:

```sh
CARGO_PROFILE_RELEASE_DEBUG=line-tables-only cargo build --locked --release -p junto-client --bin junto -p junto-client-core --bin junto-recover
python packaging/windows.py target/release/junto.exe target/release/junto-recover.exe <version> <new output directory>
```

The output directory holds the ZIP, the source directory `package-directory`, and `Junto-<version>-windows-x86_64.pdb`, the symbols of `junto.exe` with the line tables of every crate. The package does not carry them; publish the file with the release (ADR 0755, ADR 0757). `windows.py` refuses a build without symbols. The build script of the client puts the icon into `junto.exe` as its icon group 1 (ADR 0756). Publish it with the release tool of the same checkout, on any system:

```sh
target/release/junto-release publish windows directory x86_64 <output>/package-directory \
    /absolute/new-release-directory /absolute/offline/release.seed /absolute/freshness/freshness.seed
```

## Freshness

Refresh your retained, known release files before expiry, even without a new release:

```sh
target/release/junto-release refresh /private/releases/latest-macos-bundle-aarch64.json \
    /private/releases/junto-2.0.0-macos-bundle-aarch64.junto 2.0.0 /private/freshness/freshness.seed
```

The named version is mandatory. The tool verifies release signatures, version, archive size, hash and signature with its compiled trust sets before atomically replacing only freshness information. It never needs a release private key. Never refresh files fetched back from the release host. Upload the resulting metadata through the release process. Keep renewal scheduled before 30 days; a missed date or incorrect client clock prevents updating until freshness is valid again. A client refuses a statement that expires more than 45 days after its clock.

For release-key rotation, build clients with both public keys and pass additional release-secret paths after the freshness secret of `publish` or `release.sh`; both keys then sign the release and archive. The lower-level `package SOURCE OUTPUT RELEASE-SECRET FRESHNESS-SECRET VERSION SYSTEM PACKAGE ARCH [RELEASE-SECRET...]` supports the same overlap. For freshness-key rotation, include both freshness public keys and pass both freshness secrets to `refresh` before publishing. Keep signatures for every supported old trust set; an overlap client alone does not reach users who skipped that version.

## Licences

Each package holds `licences/Junto-LICENSE.txt` and `licences/third-party-licences.txt` (in the macOS bundle under `Contents/Resources`). `licences.py` writes the second file during the assembly of the package: every crate of the programs `junto` and `junto-recover` for the target of the package, with its licence and notice texts, the WebRTC library, the Rust standard library, the fonts, on Linux the bundled libraries, and in the AppImage the components of its runtime, whose source the AppImage holds in `licences/appimage-runtime` (ADR 0801). The line of a crate states when its licence files do not hold the text of its declared licence, or hold the text of a licence that it does not declare. It runs `cargo tree` and `cargo metadata` in the checkout and reads the licence file of the WebRTC library from the build directory of the programs, so assemble a package from the checkout and the build that made its programs. `python3 packaging/licences.py client <system> <architecture> <build directory> <new directory>` writes the two files alone; with `server`, the files of the server image, which its Dockerfile writes. A crate whose licence files do not hold the text of its declared licence, and whose licence has no text in `licence-texts/`, stops the assembly; add the text there (`licence-texts/README.md`).

## Test configurations and checks

External test-only public-key, freshness-key, certificate and version variables mark the build as "test configuration" in Version and `--version`. `publish` of such a build requires `--test-configuration`; a product build refuses that flag for every command. The public release wrappers refuse overrides. Tests never rewrite tracked trust or version files. `JUNTO_TEST_KEYCHAIN_LOCK` selects an isolated lock for controlled tests; every unit test sets it.

CI runs the portable Rust release, installer and configuration tests and the Python unit tests on every system. `check_signing.py` compares certificate requirements using paths with spaces and checks keychain restoration. `check_update.py` builds the release tool and the recovery program with test keys and a test certificate, then runs a real update of signed test bundles from a local source and the documented recovery command while the new executable fails; these fixture executables only end with a status. It then runs `check_lifecycle.py` on the real client: two builds of the checkout with temporary test keys, the workspace version and the next patch version, in the target directory `lifecycle` inside the target directory. The first package is installed in a folder whose name holds a space and a character outside ASCII; the installed client updates itself from a local source with `--update-from`, which only a test configuration accepts; the recovery program rolls the update back. At each of the three stages the installation must equal the package of the expected version file by file (on macOS also `codesign --verify --deep --strict`), and the client must state that version and draw its first frame with a new empty profile. `check_linux.py` (in the Linux build image) and `check_windows.py` check the Linux and Windows packages (ADR 0754, ADR 0755) and run the same lifecycle; Linux starts under X11 and Wayland and uses FUSE for the AppImage when its prerequisites are there, else states why it uses extract-and-run. The processes of every command are ended when it returns. These checks build in the repository's target directory; a local caller holds `tools/serial.sh`.

`check_release.py`, under the shared lock, builds two real macOS product releases with temporary role-separated keys. It checks test-build marking, publication refusal, compiled-source installation and the documented recovery command while the new executable fails. It removes temporary identities and files; remove the release build output afterwards. `check_start.py <Junto.app>` is separate: it starts the bundled executable with an empty temporary data directory, `--no-activate` and `--check-startup`, and requires the first-frame line. Neither packaging nor signing grants microphone, camera, screen or notification permission.

## The release site

`site.py` writes the static files of the release site from the retained release directories (ADR 0727). It needs Python 3.11 or later and no build:

```sh
python3 packaging/site.py /private/releases docs/releases /private/new-site \
    --base https://releases.example.test/junto/ \
    --server-install https://docs.example.test/junto/install.html
```

- The first argument is a directory with one directory per published version, named with the version (`0.1.0`), with every file of that release from every system: the update files of each target and the first-install files `Junto-<version>-macos-aarch64.dmg`, `Junto-<version>-macos-aarch64.zip`, `Junto-<version>-windows-x86_64-setup.exe`, `Junto-<version>-windows-x86_64.zip`, `Junto-<version>-windows-x86_64.pdb`, `Junto-<version>-x86_64.AppImage`, `junto-<version>-linux-x86_64.tar.gz` and `junto_<version>_amd64.deb`. Other directories are ignored; an unknown file stops the command. The newest version must hold every first-install file and at least one latest file.
- The second argument is the directory of the notes, `docs/releases`, with `<version>.md` for every version.
- The third argument is the new output directory. It must not exist; a failure leaves no output directory. The command does not change the releases directory.
- `--base` is the HTTPS address of the site, ending in `/`. `<base>updates/` is the update source, which every build of the release takes as `JUNTO_UPDATE_SOURCE`. `--server-install` is the address of the server install document. Neither address is written in a tracked file.

The output holds `index.html`, `notes/<version>.html`, `download/<version>/` with the first-install files, the symbols file and `SHA256SUMS`, and `updates/` with the archives and release files of every version and the newest latest file of each target. The command prints every file with its size and, last, the order of upload: every file except `updates/latest-*.json`, then the latest files.
