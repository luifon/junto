# 0726. Update archive, installation record and replacement

## Status

Accepted

The record design replaced, before the first release, a design that packed the installed application again and required a release proof beside it. The Linux and Windows steps are in ADR 0754 and ADR 0755.

## Context

An update archive is untrusted until its signature is verified. Retaining a second application bundle with the same identifier can let macOS start the wrong version. Recovery must also work when the new executable or its code signature is damaged.

The first design kept the previous version by packing the installed bundle again and requiring the result to equal the signed archive of that version, with the signed release information of the installed version in a hidden directory of the first-install folder. A person who moved only `Junto.app` lost that directory, and a file that the file manager adds inside a bundle changed the packed bytes. Both stopped every later update, although neither check protects the update: the candidate is verified without them, and they served the rollback only.

## Decision

The archive:

- The portable uncompressed `.junto` format begins with `JUNTO-ARCHIVE-1` and a line feed. Each regular file has a two-byte big-endian ASCII path length, relative path, one mode byte (0=file, 1=executable), eight-byte big-endian content length, and content. A zero path length ends the archive; trailing bytes are refused.
- Paths use `/`, safe ASCII components of at most 128 bytes and total length at most 1024 bytes. Refuse traversal, links, special file types, case-insensitive duplicates, Windows device names, trailing dots, more than 10,000 files or more than 512 MiB of content. Extraction creates a private destination, explicitly sets directories to 0755 and files to 0644/0755 independently of umask, and checks modes before exchange. Failure removes the new destination. One file can be extracted alone; the rest of the archive is read and checked in the same way.
- The archive of each package holds fixed top-level entries: `Junto.app` (macOS bundle); `junto` (Linux directory); `Junto` (Windows directory); `Junto.AppImage`, `junto-recover` and `junto-package.json` (AppImage). A package without a system-stated version carries `junto-package.json` with its version and target; the installer requires it to name the offered version and its own target.

The record beside the application:

- `.<application name>.junto-update` beside the application (for example `.Junto.app.junto-update`) holds the lock file `lock`, the record `record`, the staging directories `stage-<random>` and the downloads `download-<random>`. It is created with mode 0700; the downloads, release information and archives are 0600. It is on the file system of the application, so that every move into place is a rename. Its name does not end in `.app`, so macOS does not treat it as an application, and each installation in a directory has its own.
- `record/installed` holds the archive and the release information of the installed release, as the update downloaded and verified them. `record/previous` holds the archive and the release information of the release that the update replaced, and the recovery program, extracted from that archive. No application bundle is kept there.
- The installed application is never packed again, and no file beside it is required for an update. Files that appear inside the installed application do not affect an update.
- The source keeps the release information of each version as `release-<version>-<system>-<package>-<architecture>.json` beside its archive (ADR 0724). A record whose release information or archive does not verify is not used. When the record does not hold the installed version, the update downloads that information and archive as well, verified with the release keys only and required to name the installed version and target. When the source does not give them, or they do not verify, the update still installs the new release and states that no previous version is kept and why. The update never depends on the rollback copy.
- The first-install packages hold the application alone: the macOS ZIP holds `Junto.app`, and a person can move it alone.
- The record is not in the data directory (ADR 0710) or in another per-user directory such as `$XDG_STATE_HOME`. Staging must be on the file system of the application, so that the exchange is a rename; a record that belongs to one installation then stays with it, and two installations of one account do not share a record; the recovery program is found beside the application that it restores. A record that is lost because the application was moved costs one more download at the next update.

Installation:

- `Installer::install` takes a `VerifiedArchive` and the release of the installed version from the source, if one was needed. It holds a nonblocking exclusive lock in the update directory.
- Order: target (system, package, architecture) and "newer than the installed version" first, before any extraction or system tool; then the extraction of the verified archive, the entries, the stated version, the modes and, for a bundle, a designated requirement that equals the product identifier with the compiled certificate pin, and the candidate's platform signature. The installed bundle is not asked for its signature: a file added to it or a damaged signature does not stop an update. The pin is the requirement that every authentic installed bundle has; a build without a pin installs no bundle.
- The new record is prepared in the staging directory: the download moves into `installed` and is closed, and the record of the replaced version is linked from the old record or moved from its download, verified again, and its recovery program extracted.
- Commit: the old record moves into staging, the new record into place, then the system tool exchanges the candidate and the application (`renameatx_np` on macOS, ADR 0754 and ADR 0755 for the others). An ordinary failure before the exchange restores the old record and leaves the application unchanged. When the restore itself fails, or the exchange stops and cannot be undone, the staging directory is kept for manual recovery.
- After a finished operation the staging directory, which then holds the replaced application and the old record, is removed. A file that the system does not let go (a running program on Windows) keeps it; a mark `finished-<staging name>` beside it lets the next update or rollback remove it. Staging without a mark is never removed automatically: after a crash it can hold the only recovery copy.

Rollback:

- With every instance closed, the person runs `.<application name>.junto-update/record/previous/recover rollback /absolute/path/to/the/application` (`recover.exe` on Windows). The recovery program `junto-recover` has this one command and uses its compiled release keys and certificate pin. It never executes the current application.
- It verifies both release informations of the record with the release keys, requires the previous version to be older than the installed release, verifies the archive bytes, extracts privately, checks the entries, version and modes, and for a bundle the certificate pin and the platform signature, then exchanges. Current code-signature validity is not required. Freshness is not required for this historical, locally retained release information.
- If the version of the current application can be read, it must equal the installed release of the record. When it cannot be read (a damaged bundle, a removed description, or an AppImage outside its running program), this comparison is skipped: an older authentic release of the record can then replace that application. The other checks still require an authentic older release for this target, and for a bundle the pinned certificate.
- A successful rollback makes the restored release the installed release of the record and removes the record of the replaced release, so another rollback finds no older release and an update keeps the restored release without a download. Recovery changes no user data and does not bypass older clients' refusal of newer data formats.
- The client's `--rollback-update` performs the same rollback from the running installation.

Location:

- Startup checks location writability without creating a file. Installation makes the actual write attempt after the click. Read-only and translocated locations offer no installation. No step elevates privileges or restarts the application automatically.

Tests:

- Portable tests exercise decisions and failures behind the system-tools interface, for every package. The macOS CI step performs the documented recovery command with a new executable that fails immediately, and rejects a changed recovery archive. Linux and Windows run their own checks (ADR 0754, ADR 0755).

## Consequences

- No second retained bundle can be registered by its application identifier; the retained form is an archive and a recovery program. An in-progress extraction can temporarily contain a bundle in a private staging directory.
- The archives of the installed and the previous release need space beside the application, in addition to the candidate during an update. The first update of an installation downloads the installed version once more, unless the source no longer holds it.
- The lock file and the record remain for later operations. Download orphans may consume up to the archive limit each; automatic orphan cleanup is not implemented.
- Atomic exchange does not establish power-loss durability. Other instances are not tracked; the documented requirement to quit them remains.
- A source that withholds the release of the installed version prevents only the rollback copy, which the person is told about; it cannot change what is installed.
