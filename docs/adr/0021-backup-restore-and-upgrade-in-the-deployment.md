# 0021. Backup, restore, upgrade and the way back in the deployment

## Status

Accepted

Open: the scripts were run in the local form only, with recordings that the test wrote as rows and files. An upgrade over more than one migration, and a restore with a release that is newer than the backup, were not run.

## Context

ADR 0018 states what the server binary provides: `backup-begin`, `backup-files`, `backup-end`, `restored`, `health`, a copy of the database before a migration, and a directory for exports. The scripts of the deployment (ADR 0015) were written before these existed. They stopped the server for a backup, ended without a backup while a recording ran, ran `new-event-epoch` after a restore, and the install document named a backup as the only way back after an upgrade. The health check of the server image was a script. A restore that was interrupted while it replaced the data left a mix of old and new data on which a server could be started.

A second container of the server service cannot start beside a running server in the forms that give the server a fixed address. The image of the server has no `pg_dump`. The server deletes the directory of a removed attachment at a maintenance pass at least 60 seconds after the removal, and the directory of a removed recording through its queue of removals. The transcription worker writes the directory of a run into the directory of a finished recording.

## Decision

Backup, `deploy/backup.sh`:

- The server is not stopped. The steps are, in this order: `junto-server backup-begin`, `pg_dump` in the custom format, `junto-server backup-files`, the archive of the files, `junto-server backup-end`.
- A command of the server binary runs in the container of the server while that container runs, and otherwise in a container of its own of the same service.
- The archive holds the two directories `attachments` and `recordings` and, below them, the directories of the `file` lines that are on the disk.
- The file `left-out` of the backup has one line for each directory that the list of the server names and the archive does not hold: the `skip` lines of the server, and `absent <directory>` for a `file` line whose directory is not on the disk. The list is made from the state after the copy of the database, so a `skip` line can name an upload that the copy does not have.
- An absent directory of a recording is no failure: the list names every recording that is not running, also one that failed and has no file. An absent directory of an attachment is a lost file, because the list names an attachment only while it is complete or its directory exists: the backup is written and the script ends with the code 4.
- `tar` ends with the code 1 when a file changed or went away while it was read. That code is accepted and the lines of `tar` are shown, because the server writes parts of uploads and directories of transcription runs beside complete files. Every other code fails the backup. The script also requires that every directory that it gave to `tar` is a member of the archive, that the archive ends as `tar` ends an archive, and that no member is a link or another thing than a file or a directory.
- After the archive the script asks for the list again. That command fails for a backup that has ended, and the server ends a backup by itself after 24 hours (ADR 0018). A backup that ended before its files were copied fails.
- A failure after `backup-begin` runs `backup-end` and removes what was written.
- The backup is `database.dump`, `files.tar`, `env`, `left-out` and `SHA256SUMS`, readable for its owner only.
- A recording that runs does not prevent a backup. Its directory is a `skip` line.
- Not in the backup: the exports, the spool of transcription jobs, the certificates of the entry, the keys of the link, and the copy of the database from before a migration.

Restore, `deploy/restore.sh`:

- Before a container is stopped: the list of checksums names the dump, the archive and the settings, the checksums are right, and the archive holds no member outside the two directories and no link.
- Each container that the script starts has a label. A container with that label that an earlier run left, for example a run that was ended by force while it unpacked the archive, is ended before the staging starts.
- The staging also starts a container of the recorder once, because the replacement needs one; a host on which it cannot start fails before anything is replaced.
- The message of a failure follows the state of the database server at that moment: whether a database `junto` and a database `junto_restore` exist. A command can report a failure after its change was made.
- Staging: the database user gets the password of the settings file; the dump is loaded into the database `junto_restore`; `junto-server restored` runs on `junto_restore`; the archive is unpacked into a directory `.restore` of each volume. A failure removes the staged parts, and the data of the deployment is as it was.
- The script has no path for a database that `restored` does not accept. A backup is restored with the release that wrote it.
- Replacement, in this order: the database `junto` is deleted; the files of each volume are replaced by the staged ones; the directory of the exports, the spool of transcription jobs and the temporary directory of the recorder are emptied; each copy of the database from before a migration, recognised by its name and its comment, is deleted; `junto_restore` is renamed to `junto`.
- The four deletions remove what belonged to the replaced data and to no row of the restored database: a job in the spool has no server process that waits for it, the recorder would save a partial file of the temporary directory into the restored recordings at its next start (ADR 0015), and the way back of an upgrade would select a copy of the replaced database.
- Between the deletion and the rename no database `junto` exists, so a server does not start. This is the only mark of a restore that has not ended; the script writes no marker file.
- Every run restores everything from the backup. A run after an interrupted run needs no state of the run before.

Upgrade:

- The copy before a migration is on (`JUNTO_MIGRATION_COPY`, default `database`). The database user of the Compose files is the first user of the PostgreSQL container, which may create a database.
- The procedure of the install document is: a stop of the server, `check-upgrade`, a backup with the server stopped, a check of the free space for the copy, the new checkout, build and start. The backup follows the stop, so that it holds everything up to the upgrade and no recording that ran.
- `deploy/upgrade-undo.sh` is the way back to the copy. It runs in the checkout of the earlier release. It stops the server and the worker, builds the server image of that checkout and runs `new-event-epoch` on the copy, because the clients hold positions of events that the copy does not have (ADR 0010). Then, in one transaction, it renames `junto` to `junto_undone`, renames the copy to `junto` and removes the comment that marks a copy. The epoch comes before the renames, so that no database with the name `junto` has the earlier epoch, and a failure leaves every name as it was.
- Without a name, the script takes the one database `junto_before_<number>` that has the comment of the server. With a name, it takes that database when its name has this form.
- The script changes nothing while a database `junto_undone` exists. It does not delete it: the deletion of a database is not part of a transaction, and a rename that fails afterwards would leave neither.
- The copy is the way back while the newer version deleted no file. Otherwise the way back is the backup that the procedure makes first.

Exports:

- The server gets `JUNTO_EXPORTS_DIR` and a volume `exports` of its own, with the owner and the mode of the attachments. `init` prepares it.

Health check:

- The health check of the server image is `junto-server health` (ADR 0012). The script `junto-healthcheck` is removed.

Trusted proxy:

- `JUNTO_TRUSTED_PROXIES` stays one address in the one-host form and on the data host: the address of the host in `backend`, from which the entry connects, or the address of the link (ADR 0015). The local form has no proxy and sets none. The setting takes a network; a network of `backend` would also trust the recorder and Redis.

## Consequences

- The server serves during a backup. A file that is deleted during a backup stays on the disk until the backup ends.
- An archive can hold a directory that its database does not name: an upload or a recording that ended between the copy of the database and the list. After a restore that directory stays on the disk, and nothing deletes it. The server has no command that removes directories that the database does not name.
- A recording that runs or is processed during a backup is not in the backup. The operator reads `left-out`.
- A restore needs free space for a second copy of the files and of the database until the replacement.
- While a restore replaces the data, and after an interruption of that step, the log of a server that is started states that the database `junto` does not exist. It does not name the restore.
- A restore with a newer release than the backup depends on the `restored` command of that release. A restore with an older release than the backup replaces the data, and the server then does not start on the newer schema; the release of the backup starts on it.
- A restore needs the image of the recorder, because a container of the recorder service empties its temporary directory.
- A checkout that is older than `deploy/upgrade-undo.sh` gets the script of the newer release by hand.
- The scripts hold the text of the comment that the server gives a copy. A change of that text in the server needs a change of the two scripts; the upgrade test fails otherwise.
- A stray link below the attachments or the recordings fails every backup until it is removed.
- An upgrade with a migration needs free space of the size of the database in the volume of PostgreSQL. The storage guard does not count it.
- The container of a server that fails at its start is started again by its restart rule. Each start with a waiting migration makes the copy again and deletes the copy before it (ADR 0018). A stop of the server between the creation of the copy and its comment leaves a copy without the comment; the next start then fails, because a database of that name exists and the server does not delete what it did not mark. After a failure in the second of several migrations, the next start replaces the copy of the earlier schema with a copy of the schema after the first migration; the earlier release does not start on that. The backup before the upgrade is the way back then.
- After the way back, the content that was written after the upgrade is in `junto_undone` only, and a directory that was written after the upgrade stays on the disk without a row.
- The server connects to PostgreSQL as the first user of the container, which has every right in that PostgreSQL server.
