# 0726. Update archive and macOS replacement

## Status

Accepted

## Context

An update archive is untrusted until its signature is verified. Retaining a second application bundle with the same identifier can let the system start the wrong version. Recovery must also work when the new executable or its code signature is damaged.

## Decision

- The first-install ZIP contains a `Junto` directory with `Junto.app` and its detached signed release proof in `.Junto.releases/<version>.json`. Move that complete directory. The bundle includes the standalone `Contents/MacOS/junto-release` recovery tool, signed with the same certificate before the enclosing bundle is signed.
- The portable uncompressed `.junto` format begins with `JUNTO-ARCHIVE-1` and a line feed. Each regular file has a two-byte big-endian ASCII path length, relative path, one mode byte (0=file, 1=executable), eight-byte big-endian content length, and content. A zero path length ends the archive; trailing bytes are refused.
- Paths use `/`, safe ASCII components of at most 128 bytes and total length at most 1024 bytes. Refuse traversal, links, special file types, case-insensitive duplicates, Windows device names, trailing dots, more than 10,000 files or more than 512 MiB of content. Extraction creates a private destination, explicitly sets directories to 0755 and files to 0644/0755 independently of umask, and checks modes before exchange. Failure removes the new destination.
- `Installer` accepts a `VerifiedArchive` and returns the recovery-data path. Linux and Windows provide their own platform installation step over this regular-file format. The macOS archive contains exactly one `Junto.app`.
- Installation holds a nonblocking exclusive lock in the application's parent. It verifies the archive before extraction or platform tools, checks target/version, the installed certificate-backed designated requirement, the extracted modes and version, exact equality of candidate and installed requirements, and the candidate's platform signature.
- Before replacing the application, pack its exact installed files into a private archive and verify that archive against the retained release proof, using trusted release keys. A missing proof or changed installed content stops the update. Prepare `.Junto.previous` with `archive.junto`, the old `release.json`, the newly installed version's `installed.json`, and a standalone `recover` executable copied from the verified installed bundle. No previous application bundle is retained there.
- Save the new detached proof before exchange. Move any existing recovery directory into staging, move the prepared recovery directory into place, then atomically exchange the candidate and `Junto.app` using `renameatx_np` through rustix. Before exchange, an ordinary failure restores the old recovery data and leaves the application unchanged. If restoration itself fails, retain staging for manual recovery. On success, remove the exchanged old bundle and obsolete recovery data from staging.
- With every instance closed, the person runs `.Junto.previous/recover rollback /absolute/path/to/Junto.app`. The tool uses its compiled release keys and certificate pin; it never executes the current application. It verifies both retained release proofs, requires the backup version to precede the installed release, verifies the archive bytes, unpacks privately, checks shape/modes/version and the certificate pin plus platform signature, then exchanges the restored bundle with the current application. Current code-signature validity is not required. Freshness is not required for this historical, locally retained release proof.
- If the current version can be read, it must equal the signed installed version before rollback. Successful rollback leaves the archive recovery data and removes the replaced broken bundle; another rollback refuses the already restored version. Recovery changes no user data and does not bypass older clients' refusal of newer data formats.
- Startup checks location writability without creating a probe file. Installation makes the actual write attempt after the click. Read-only and translocated locations offer no installation. No step elevates privileges or restarts the application automatically.
- Portable tests exercise decisions and failures behind the system-tools interface. Python unit tests on macOS perform the documented command with a new executable that fails immediately, and reject tampered recovery archives. Full product builds and GUI startup remain separate checks.

## Consequences

- No second retained bundle can be registered by its application identifier; the retained form is an archive and standalone executable. An in-progress extraction can temporarily contain a bundle in a private staging directory.
- The archive and detached proof need space in addition to the candidate and running bundle. The original installation directory must remain intact.
- The lock file and release proofs remain for later operations. After a crash, staging may contain the sole recovery copy and must be inspected before removal. Download orphans may consume up to the archive limit each; automatic orphan cleanup is not implemented.
- Atomic exchange does not establish power-loss durability. Other instances are not tracked; the documented requirement to quit them remains.
