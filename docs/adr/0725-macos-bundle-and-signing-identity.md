# 0725. macOS bundle and signing identity

## Status

Accepted

## Context

ADR 0704 requires one bundle identifier and one self-signed code-signing certificate across updates. The identity spike used identifiers for test applications, not for the product. A command-line executable has no bundle metadata for the operating system's permission and notification interfaces.

## Decision

- The product identifier is `org.junto.app`. It never changes. The bundle is `Junto.app`, its executable is `Contents/MacOS/junto`, and both version fields are the workspace package version.
- `packaging/bundle.py` assembles the release executable, the recovery program `junto-recover` (`Contents/MacOS/junto-recover`, ADR 0726), an Info.plist and an icon into a new directory. It refuses an existing destination. The icon file `Contents/Resources/Junto.icns` holds the application icon at each size of macOS; the program `junto-icon` of the same build writes it from the SVG source, and `bundle.py` takes that program as its fourth argument (ADR 0756). `Contents/Resources/licences/` holds the licence of Junto and the licence file of the bundle (ADR 0801).
- The bundle states why the microphone, camera and screen are used. These texts do not request permission. System notifications require execution as this bundle and notification permission; packaging alone does not grant permission or implement notification delivery.
- `packaging/create_identity.py` creates a ten-year self-signed RSA code-signing certificate and its private key in a new private directory outside a repository. It imports them into a dedicated keychain and enables codesign to use that key without a prompt. It never changes the default keychain or system trust.
- The certificate is made by the OpenSSL of the system, `/usr/bin/openssl`, with the configuration file `packaging/code-signing.cnf`, which states the name and every extension. The first `openssl` in PATH can be another version with another default configuration: the default configuration of OpenSSL 1.1 adds `basicConstraints = critical,CA:true` to a self-signed certificate, the requested `CA:FALSE` is then a second basic constraints extension, and `codesign` finds no identity for that certificate. A test signs with an identity made while `OPENSSL_CONF` names such a configuration.
- `packaging/sign.sh` takes the certificate identity and dedicated keychain from `JUNTO_CODESIGN_IDENTITY` and `JUNTO_CODESIGN_KEYCHAIN`. Empty and ad-hoc identities are refused. It signs without a timestamp, verifies the signature and requires a designated requirement that includes the certificate.
- The certificate must be visible in the user's keychain search list while signing. Each command captures that list before changing it and restores the exact list in its cleanup path. A fixed per-user lock serializes search-list changes independently of temporary-directory settings; handled interrupt, termination and hangup signals cannot interrupt restoration. Checks restore the list captured before any child signer ran, including after a killed signer. The certificate is not installed on recipients' machines.
- The operator keeps the certificate, private key, dedicated keychain and password outside repositories, with private access and an encrypted offline backup. A replacement certificate changes the application's permission identity even if its private key stays the same.
- `packaging/certificate.sha1` pins the permanent certificate fingerprint. It is empty before the operator creates the identity; signing and release publication refuse an empty pin or any different certificate. Publication and recovery require the complete designated requirement to equal the product identifier followed by the pinned leaf-certificate condition; merely mentioning the fingerprint inside a more permissive expression is insufficient. Tests use an external pin for that build only.

## Consequences

- Two test bundles with different version fields and one test certificate have equal designated requirements. An ad-hoc signature has a different requirement. The identity comparison uses temporary fixture bundles.
- A release certificate expiring requires an explicit decision about permission identity; automatic update signing key rotation cannot repair it.
- The first launch of a downloaded copy remains blocked as described by ADR 0704. A quarantined downloaded product bundle, a clean-system install and real system notifications remain untested.
- `packaging/check_start.py` starts the executable of a bundle with an empty temporary data directory, `--no-activate` and `--check-startup`, and requires exit code 0 and the line `Junto first frame rendered`. The macOS CI job assembles the unsigned bundle of the debug build of each commit and runs this check on it (ADR 0905). `packaging/check_update.py` also checks the signed real-client lifecycle below.


Package lifecycle check (`packaging/check_update.py`, `packaging/check_lifecycle.py`):

- Build the real client, recovery program and release tool from the same checkout twice,
  first with the workspace version and then with the patch version raised by one. Compile
  temporary release and freshness public keys into both builds with the test configuration;
  use the temporary signing identity and certificate pin of the packaging checks. No tracked
  key, certificate pin, manifest or lockfile is changed.
- Assemble and sign both bundles, create the first-install ZIP with `ditto`, and extract it
  into the temporary installation directory. Start the installed client with an empty data
  directory, `--no-activate` and `--check-startup`. Require its compiled version, exit code 0
  and the first-frame line.
- Serve both signed releases on a loopback source. Update with the first build's release
  tool, start the updated client with another empty data directory, roll back with the
  recovery program kept in the previous release record, and start the restored client with
  a third empty data directory. Check the compiled version at each stage and the same
  designated signing requirement throughout.
- Each start has a 120-second bound. A timeout terminates the process group and reports the
  captured output. Interrupt, termination and hangup signals stop the active command group.
  Unix commands inherit the CI run lock descriptor so an interrupted runner cannot release
  its build cache while a command still uses it (ADR 0905). Temporary packages, profiles,
  keys and release files are removed, and the
  keychain search list is restored. The checks use no real release key or signing identity.
- Fixture checks remain for invalid signatures, a read-only installation, a damaged archive
  and a second rollback. Download quarantine, system permission continuity and a clean
  machine remain separate checks.
