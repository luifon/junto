# 0021. Backup, restore, upgrade and the way back in the deployment

## Status

Accepted

On 2026-10-16 a backup and isolated restore ran on the Linux data host of the two-host form with actual recorder output. All original audit rows, other table counts and six stored-file checksums matched; the restored server was healthy and added exactly one expected epoch-renewal audit row. Temporary image aliases supplied the built images under the isolated project's names. Open: a public one-host backup and restore, and upgrade outside the local form. The earlier local tests use recordings represented by rows and files. The way back to a release that has the command `unnamed-files` was run with a copy that the test made, because no later release exists.

## Context

ADR 0018 states what the server binary provides: `backup-begin`, `backup-files`, `backup-end`, `restored`, `schema-version`, `unnamed-files`, `health`, a copy of the database before a migration, and a directory for exports. The scripts of the deployment (ADR 0015) were written before these existed. They stopped the server for a backup, ended without a backup while a recording ran, ran `new-event-epoch` after a restore, and the install document named a backup as the only way back after an upgrade. The health check of the server image was a script. A restore that was interrupted while it replaced the data left a mix of old and new data on which a server could be started.

A second container of the server service cannot start beside a running server in the forms that give the server a fixed address. The image of the server has no `pg_dump`. The server deletes the directory of a removed attachment at a maintenance pass at least 60 seconds after the removal, and the directory of a removed recording through its queue of removals. The transcription worker writes the directory of a run into the directory of a finished recording.

## Decision

Backup, `deploy/backup.sh`:

- The server is not stopped. The steps are, in this order: `junto-server schema-version`, `junto-server unnamed-files --remove`, `junto-server backup-begin`, `pg_dump` in the custom format, `junto-server backup-files`, the archive of the files, `junto-server backup-end`.
- The removal of the directories that no row names is part of the backup, because a backup is the one script that an operator runs at regular times, and the command deletes nothing while a backup holds deletions back, so it runs before `backup-begin`. A failure of the removal is printed and does not stop the backup. The script prints each directory that was removed.
- The file `schema` of the backup holds the line of `schema-version`: the newest migration of the server binary, which is the schema of the database of a server that runs (ADR 0018). While the server is stopped, the commands run in a container of the server image, and that image can be of another release than the database. The script compares the line with the newest migration that the database states as applied, and makes no backup when the two differ: the commands of the binary are written for its own schema, and a backup with a wrong line could be restored by no release.
- `pg_dump` runs as the role of the server (ADR 0022).
- A command of the server binary runs in the container of the server while that container runs, and otherwise in a container of its own of the same service.
- The archive holds the two directories `attachments` and `recordings` and, below them, the directories of the `file` lines that are on the disk.
- The file `left-out` of the backup has one line for each directory that the list of the server names and the archive does not hold: the `skip` lines of the server, and `absent <directory>` for a `file` line whose directory is not on the disk. The list is made from the state after the copy of the database, so a `skip` line can name an upload that the copy does not have.
- An absent directory is a lost file: the list names an attachment only while it is complete or its directory exists, and a recording only when it has files (ADR 0018). The backup is written with what exists, and the script ends with the code 4, for an attachment and for a recording.
- `tar` ends with the code 1 when a file changed or went away while it was read. That code is accepted and the lines of `tar` are shown, because the server writes parts of uploads and directories of transcription runs beside complete files. Every other code fails the backup. The script also requires that every directory that it gave to `tar` is a member of the archive, that the archive ends as `tar` ends an archive, and that no member is a link or another thing than a file or a directory.
- `backup-end` fails for a backup that has expired: the server ends a backup by itself after 24 hours (ADR 0018). The script then writes no backup. It reads the words `expired at` in the message of the server to tell this case from a server that could not be asked; in the second case it calls `backup-end` once more at its end and names the command when that fails too.
- A failure after `backup-begin` runs `backup-end` and removes what was written.
- The backup is `database.dump`, `files.tar`, `env`, `left-out`, `schema` and `SHA256SUMS`, readable for its owner only.
- A recording that runs does not prevent a backup. Its directory is a `skip` line.
- Not in the backup: the exports, the spool of transcription jobs, the certificates of the entry, the keys of the link, and the copy of the database from before a migration.

Restore, `deploy/restore.sh`:

- Before a container is stopped: the list of checksums names the dump, the archive, the settings and the file `schema` when the backup has one, the checksums are right, and the archive holds no member outside the two directories and no link.
- Before a container is stopped: the schema of the backup is the schema that `junto-server schema-version` of the server image states. A backup of a newer and of an earlier schema is refused, with both numbers. The command `restored` accepts the schema of its own binary only, and a server does not start on a newer schema, so no other backup can be restored by this image.
- The image is the one that the staging runs: the image of the service `server` by its name, in a container without a network, with a name of the project and the limits of `init` (ADR 0015). A server that runs can be of another image, when the image was built again after its start, and a second container of the service cannot start beside it in the forms that give the server a fixed address. The image is built when it does not exist.
- A backup without the file `schema`, which the first release with the backup commands wrote, is staged, and `restored` decides: its message names both schemas, and nothing is replaced.
- Each container that the script starts has a label. A container with that label that an earlier run left, for example a run that was ended by force while it unpacked the archive, is ended before the staging starts.
- The staging also starts a container of the recorder once, because the replacement needs one; a host on which it cannot start fails before anything is replaced.
- The message of a failure follows the state of the database server at that moment: whether a database `junto` and a database `junto_restore` exist. A command can report a failure after its change was made.
- Staging: the database `junto_restore` is created; the role file runs in it, so the role of the server exists, has the password of the settings file and owns the database (ADR 0022); the role loads the dump; `junto-server restored` runs on `junto_restore`; the archive is unpacked into a directory `.restore` of each volume. A failure removes the staged parts, and the data of the deployment is as it was.
- The script has no path for a database that `restored` does not accept. A backup is restored with the release that wrote it.
- Replacement, in this order: a database that a stopped start of the server left without a comment is deleted; the database `junto` is deleted; the files of each volume are replaced by the staged ones; the directory of the exports, the spool of transcription jobs and the temporary directory of the recorder are emptied; each copy of the database from before a migration, and each database that a stopped start left under the name `junto_making_<number>`, is deleted; `junto_restore` is renamed to `junto`. A copy is recognised by its name and the comment of a copy. A database that a stopped start left without a comment is recognised by the comment of the database `junto` alone (ADR 0018), so it is deleted before that database: a run that is ended between the two deletions leaves no database that only a deleted comment named.
- After the rename, a failure or an end of the script states that the data was replaced and the restore is complete.
- After the rename the script runs `junto-server unnamed-files` without `--remove` and prints its lines, with the rule of the 24 hours. It removes nothing: the operator reads the list first, and the next backup removes what is old enough. A failure of this command is a message, because the restore is complete.
- The four deletions remove what belonged to the replaced data and to no row of the restored database: a job in the spool has no server process that waits for it, the recorder would save a partial file of the temporary directory into the restored recordings at its next start (ADR 0015), and the way back of an upgrade would select a copy of the replaced database.
- Between the deletion and the rename no database `junto` exists, so a server does not start. This is the only mark of a restore that has not ended; the script writes no marker file.
- Every run restores everything from the backup. A run after an interrupted run needs no state of the run before.

Upgrade:

- The copy before a migration is on (`JUNTO_MIGRATION_COPY`, default `database`). The role of the server owns its database and may create one (ADR 0022).
- The procedure of the install document is: a stop of the server, `check-upgrade`, a backup with the server stopped, a check of the free space for the copy, the new checkout, `deploy/database-user.sh` for a deployment that does not have the role of the server, build and start. The backup follows the stop, so that it holds everything up to the upgrade and no recording that ran.
- `deploy/upgrade-undo.sh` is the way back to the copy. It runs in the checkout of the earlier release. It stops the server and the worker, builds the server image of that checkout, gives the database role of that server the password of the settings file (ADR 0022), and runs `new-event-epoch` on the copy, because the clients hold positions of events that the copy does not have (ADR 0010). Then, in one transaction, it renames `junto` to `junto_undone`, renames the copy to `junto` and removes the comment that marks a copy. The epoch comes before the renames, so that no database with the name `junto` has the earlier epoch, and a failure leaves every name as it was.
- Without a name, the script takes the copy that the first line of the comment of `junto` names. The server writes that line when the copy exists and removes it when every migration ran (ADR 0018), so it is there after an upgrade that failed, and the copy that it names is the one from before the first migration. Without that line, the script takes the one database `junto_before_<number>` that has the comment of a copy. With a name, it takes that database when its name has this form.
- When the comment names a copy that no longer exists, the script changes nothing and names the backup as the way back.
- The line of the unfinished upgrade stays on the database that was migrated in part, which is `junto_undone` after the renames. The copy loses its comment and is `junto`.
- A database `junto_making_<number>` with the comment of a copy, or without a comment and named by the comment of `junto`, is what a start of the server left that was stopped while it made its copy. The server deletes it at its next start by the comment of `junto`; after the renames that comment is on `junto_undone`. The script prints each such database with the command that deletes it, and deletes none. Another database of such a name is not mentioned.
- After the renames the script runs `junto-server unnamed-files` of the checkout without `--remove` and prints its lines. A release before that command prints its usage; the script states that, and the directories stay.
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
- An archive can hold a directory that its database does not name: an upload or a recording that ended between the copy of the database and the list. After a restore that directory stays on the disk until a backup, or the command by hand, removes it, at least 24 hours after its last change. `tar` restores the time of the last change of a directory, so the age is counted from before the backup.
- A backup deletes directories: the ones that no row names and that are older than 24 hours. A directory that somebody put below the attachments, the recordings or the exports under a name in the form of an identifier is deleted by the next backup after that time.
- A recording that runs or is processed during a backup is not in the backup. The operator reads `left-out`.
- A restore needs free space for a second copy of the files and of the database until the replacement.
- While a restore replaces the data, and after an interruption of that step, the log of a server that is started states that the database `junto` does not exist. It does not name the restore.
- A restore needs the release of the backup: the image of the server of that checkout must exist or be built before anything is stopped, because the script asks it for its schema.
- A restore needs the image of the recorder, because a container of the recorder service empties its temporary directory.
- A checkout that is older than `deploy/upgrade-undo.sh` gets the script of the newer release by hand.
- `deploy/backup.sh` reads the table of the applied migrations of the database, with the statement of the server for the newest one.
- The scripts hold three texts of the server: the comment of a copy (`restore.sh`, `upgrade-undo.sh`, `database-user.sh`), the first line of the comment of a database whose upgrade has not finished (`upgrade-undo.sh`), and the words `expired at` of `backup-end` (`backup.sh`). A change of one of them in the server needs a change of these scripts; the two tests fail otherwise.
- A stray link below the attachments or the recordings fails every backup until it is removed.
- An upgrade with a migration needs free space of the size of the database in the volume of PostgreSQL. The storage guard does not count it.
- The container of a server that fails at its start is started again by its restart rule. A start that follows a failed migration makes no copy and deletes none (ADR 0018), so the copy from before the first migration stays over every such start.
- After the way back, the content that was written after the upgrade is in `junto_undone` only, and a directory that was written after the upgrade stays on the disk without a row until a backup, or the command by hand, removes it.
- A checkout with the earlier form of `deploy/upgrade-undo.sh` does not read the comment of the database. It takes the one copy with the comment of the server, which is the same database while the server keeps one copy.
