# macOS release tooling

The build machine needs Python 3.11 or later. `bundle.py <binary> <new Junto.app> <recovery binary>` assembles both executables and metadata using the workspace version. The fixed identifier is `org.junto.app` (ADR 0725).

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

The helper signs the recovery executable and then the bundle. It temporarily includes the dedicated keychain in the search list, restores the original list, and never changes default keychain or system trust. A fixed per-user lock serializes search-list changes independently of TMPDIR. Interrupt, termination and hangup are handled, including interruptions during restoration. Password commands use standard input; the working PEM and password files remain unencrypted secrets. Never put them in a shared folder, repository, release directory or unencrypted backup. SIGKILL and power loss cannot run cleanup: inspect the search list after either.

Create distinct release and freshness keys:

```sh
tools/serial.sh cargo run -p junto-client-core --bin junto-release -- \
    keygen /absolute/offline/release.seed /absolute/offline/release-public.json
tools/serial.sh cargo run -p junto-client-core --bin junto-release -- \
    keygen /absolute/freshness/freshness.seed /absolute/freshness/freshness-public.json
```

Each seed is generated randomly, written privately and never overwritten. Keep both outside repositories with encrypted offline backups. Copy only the release public array to `packaging/update-keys.json` and the freshness public array to `packaging/freshness-keys.json`, review and track both. Release keys authorize code and are brought online only for a new release. Keep freshness keys in a separate private signing environment for renewal before each 30-day expiry; a freshness key cannot authorize new code. The release host needs neither private key.

Create release files without uploading them:

```sh
JUNTO_UPDATE_SOURCE='https://<your-release-host>/<release-directory>/' \
JUNTO_UPDATE_KEY='/absolute/offline/release.seed' \
JUNTO_FRESHNESS_KEY='/absolute/freshness/freshness.seed' \
JUNTO_CODESIGN_IDENTITY='<certificate fingerprint>' \
JUNTO_CODESIGN_KEYCHAIN='/absolute/private/code-signing/signing.keychain-db' \
    packaging/release.sh /absolute/new-release-directory
```

Use a new absolute output directory. The command builds the version, assembles and signs the bundle, checks its identity, compiled trust and version, and writes the `.junto` archive and `latest-<system>-<architecture>.json`. The first-install ZIP holds a complete `Junto` folder with the application and its detached release proof. Users must retain that complete folder. No Python is needed on the installed machine. The internal `release_locked.sh` entry point requires its caller to hold the shared lock.

Refresh your retained, known release files before expiry, even without a new release:

```sh
target/release/junto-release refresh /private/releases/latest-macos-aarch64.json \
    /private/releases/junto-2.0.0-macos-aarch64.junto 2.0.0 /private/freshness/freshness.seed
```

The named version is mandatory. The tool verifies release signatures, version, archive size, hash and signature before atomically replacing only freshness information. It never needs a release private key. Never refresh files fetched back from the release host. Upload the resulting metadata through the release process. Keep renewal scheduled before 30 days; a missed date or incorrect client clock prevents updating until freshness is valid again.

For release-key rotation, build clients with both public keys and pass additional release-secret paths after the output directory of `release.sh`; both keys then sign the release and archive. The lower-level `package SOURCE OUTPUT RELEASE-SECRET FRESHNESS-SECRET VERSION SYSTEM ARCH [RELEASE-SECRET...]` and `publish BUNDLE OUTPUT RELEASE-SECRET FRESHNESS-SECRET [RELEASE-SECRET...]` support the same overlap. For freshness-key rotation, include both freshness public keys and pass both freshness secrets to `refresh` before publishing. Keep signatures for every supported old trust set; an overlap client alone does not reach users who skipped that version.

The `.junto` transport is the bounded regular-file format of ADR 0726. Linux/Windows packaging implement `releases::Installer` over `VerifiedArchive`; their tarball, AppImage and installer formats are separate work. The format-1 metadata filename remains format 1 forever; future incompatible formats need separate filenames.

External test-only public-key, freshness-key, certificate and version variables mark the build as "test configuration" in Version and `--version`. `publish` refuses such a build without `--test-configuration RELEASE-PUBLIC FRESHNESS-PUBLIC`; the public release wrapper refuses overrides entirely. Tests never rewrite tracked trust or version files. `JUNTO_TEST_KEYCHAIN_LOCK` selects an isolated lock only for controlled tests; ordinary signing uses the fixed per-user path.

CI runs portable Rust release/installer/configuration tests and Python unit tests. On macOS those unit tests run the real recovery command with a failing new executable, tampered archive and invalid certificate cases, plus a read-only location with zero source requests. `check_signing.py` additionally compares certificate requirements using paths with spaces and checks keychain restoration. `check_update.py target/debug/junto-release` exposes the same recovery check manually; its test executables are only `true` and `false`, with no GUI or audio.

`check_release.py`, under the shared lock, builds two real product releases with temporary role-separated keys. It checks test-build marking, publication refusal, compiled-source installation and the documented standalone recovery command while the new executable fails. It removes temporary identities/files; remove the release build output afterwards. `check_start.py <Junto.app>` is separate: it starts the bundled executable with an empty temporary data directory, `--no-activate` and `--check-startup`, and requires the first-frame line. Neither packaging nor signing grants microphone, camera, screen or notification permission.
