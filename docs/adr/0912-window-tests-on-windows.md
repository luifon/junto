# 0912. Window tests on Windows and their database

## Status

Accepted

The steps ran on the Windows machine of ADR 0905 on 2026-10-17: the 3 window tests without a database and the 97 with PostgreSQL pass (1 ignored). The two unit tests of `tools/windows` that had not run before failed there and were corrected: the version test read the PostgreSQL image from the workflow, where it no longer is (it is in `tools/ci/pins.sh`), and the test of a child that keeps the output of `pg_ctl` open left the child holding a file of the test's temporary folder, which Windows does not remove; the test now ends the child and waits for its end. The server now has native file storage on Windows (ADR 0606); the Windows job includes the file, export and picture window tests. Finished-recording fixtures still require FFmpeg and their /bin/sh transcription stand-in. Native verification of that backend is pending. The tests of `live::` need LiveKit and do not run on Windows.

## Context

The scope item "Client on Windows" requires that the client starts on Windows and that its window tests pass there. The check of the Windows package (ADR 0755) already starts the installed `junto.exe` with `--check-startup` and requires exit code 0 and the line `Junto first frame rendered`.

The window tests in `crates/client/tests/app` open the window code on the test platform of gpui, whose text system draws no glyphs; they need no display and no GPU. Most of them start the server binary of the workspace on a database of their own, from the PostgreSQL that `JUNTO_TEST_DATABASE_URL` names. On Linux, `tools/with-test-postgres.sh` starts that PostgreSQL as a Linux container. The Docker engine of a Windows runner runs Windows containers only, and service containers of GitHub Actions run on Linux runners only. The runner image `windows-2025` has a PostgreSQL installed, but its version is that of the image and changes when the image changes.

The server opens and creates its files below a directory without following links: through `openat` with `O_NOFOLLOW` on Unix, and pinned directory handles without delete sharing plus no-follow opens on Windows (`crates/server/src/recordings/files.rs`, ADR 0606). Attachments, exports, backups and recordings use these operations.

## Decision

- The Windows job gets its database from `tools/windows/with_test_postgres.py`. It downloads the PostgreSQL build for Windows of the version of `JUNTO_TEST_POSTGRES_IMAGE` (the ZIP of binaries from EnterpriseDB), checks its SHA-256 against the sum in the program, keeps the checked file in `~/.cache/junto-postgres` (on GitHub the job saves it in its cache in a step before the tests, so that a failed test does not lose the download; on the Windows machine of ADR 0905 it is in `postgres` of the CI directory, through `JUNTO_TEST_POSTGRES_DOWNLOADS`), and extracts only `bin`, `lib` and `share`. For each run it creates a new data directory with `initdb`, starts the server with `pg_ctl start -w` on the loopback address, at a free port, with the settings of `tools/with-test-postgres.sh`, runs the command with `JUNTO_TEST_DATABASE_URL`, and stops the server, also after a start that failed. A start that fails prints the output of `pg_ctl` and the log of the server. The output of `pg_ctl` goes into a file: on Windows the server keeps the output handles of `pg_ctl` while it runs, so a pipe would not reach its end until the server stops. `pg_ctl` removes the administrator rights from the token of the server, which the server requires on Windows.
- `tools/check-pins.sh` fails when the version of the program differs from the version of the CI image or when the program has no SHA-256. A unit test fails when its server settings differ from those of `tools/with-test-postgres.sh`.
- The Windows job runs the window tests in two steps after the package check: `updates::` without a database, and the other applicable cases with the database. File uploads, downloads, pictures and exports use the native Windows file store. The LiveKit tier remains on Linux. Finished-recording and transcript cases, including the export containing a recording and the transcript-ended system line, keep their exclusions: their fixture requires FFmpeg and /bin/sh, which this job does not provide. The job also runs the native file-store unit tests, including no-clobber finalization and junction refusal.
- The harness of the window tests sends the log of the client into the output of each test, at the level of `JUNTO_LOG_LEVEL` and `info` without it. The test runner prints that output for a test that fails, on every system.
- The account-switch file-dialog test runs its owner change and stale-dialog refusal on every system. Its final successful upload runs on both Unix and Windows, where the test server stores files.

## Consequences

- The tests on Windows use the PostgreSQL version of the tests on Linux. A change of the CI image needs a new ZIP with its sum.
- The first run of a new version downloads 340 MB; the next runs read the cache.
- File operations no longer require a platform exclusion. The remaining exclusions name the live media service and the Unix transcription fixture, rather than the Windows file store.
- The Windows backend is exercised by the same window assertions as the Unix backend, and by native handle and reparse-point tests (ADR 0606).
