# 0018. What the server binary does for a backup, after a restore and before a migration

## Status

Accepted

## Context

ADR 0005 states that the server provides a backup command and a restore command, that a backup lists the files it needs and the server keeps them until the backup reports its end, that files that are still written are left out and reported, and that the server copies the database before a migration. None of this existed. The scripts of the deployment copy the database with `pg_dump` and the files with `tar` while the server is stopped. The image of the server holds no `pg_dump`, the settings file of a deployment and the keys of the entry are outside the container of the server, and a program that copies a PostgreSQL database must have the version of that database. The server deletes files apart from its requests: the directories of removed recordings (ADR 0608) and of removed attachments (ADR 0408).

Every task of the server that deletes a directory starts from a row. After a restore, the files of a backup can hold a directory that the restored database does not name: an upload or a recording that began after the copy of the database and had ended when the list was made. After the way back from an upgrade, the directories that were written after the upgrade stay. No task deletes these directories.

PostgreSQL has no statement that creates a database with a comment, and a migration that fails leaves the migrations before it applied.

## Decision

Division of the work:

- The copy of the database, of the files and of the settings is made by the programs of the deployment (`pg_dump`, `tar`). The server binary provides the parts that only the server can provide. This narrows the line of ADR 0005 that both commands cover the database, the files, the configuration and the keys.

`junto-server backup-begin`:

- Stores a backup with an identifier and an expiry 24 hours later, and prints `backup <identifier> <expiry>`.
- While a backup has not ended and has not expired, the server deletes no directory of a recording and no directory of an attachment. The deletions stay in their queues.
- The server keeps running. A backup copies the database after this command.

`junto-server backup-files <identifier>`:

- Is run after the copy of the database, with the settings of the server, and prints one line for each directory: `file attachments/<identifier>` and `file recordings/<path>` for what the backup copies, and `skip <directory> <reason>` for what it leaves out: `upload_not_complete`, `recording_runs`, `files_are_processed`. The rows are read in pages.
- The list holds every file that the copy of the database states as complete. Such a file was written before the copy, and no file is deleted while the backup runs. An upload that ended after `backup-begin` is in the list, because the list is made after the copy. An attachment or a recording that was removed after the copy is in the list while its directory is on the disk: the attachment by its row, the recording by its entry in the queue of removals. So the list is read from the state after the copy, and it can hold more than the copy needs, never less.
- A recording of the state `failed` has no file (ADR 0606) and gets no line. A recording of the state `ready` or `partly_failed` gets its `file` line also when its directory is absent, so that the program that copies the directories reports the loss.
- A backup that has ended or expired has no list: the command ends with a failure.
- The command reads the disk to learn whether a removed directory is still there. When the database has attachments or recordings and the command does not see the directory of that kind, because the setting is absent or names no directory, it prints no list and ends with a failure.
- The task that deletes the directories of attachments reads the backup in the transaction that selects the directories, so a directory that is removed after the start of a backup is not selected by a run that began before.

`junto-server backup-end <identifier>`:

- Ends the backup. The deletions that waited are done at the next run of their tasks. The exit code is 0.
- A backup that has expired is not ended. The command prints the time of the expiry and ends with the exit code 1: the server may have deleted files before the backup copied them, so the program that made the backup discards it. The stored backup stays, so that a second call gives the same answer, until the next `backup-begin` removes it.
- An unknown identifier ends the command with the exit code 1. A backup that was ended is unknown, so a second `backup-end` for it is a failure.

`junto-server restored`:

- Is run after the database and the files of a backup were put in place, before the server starts.
- Gives the event stream a new epoch (ADR 0010), removes every stored backup, and removes every export.
- Exports are not part of a backup: they are copies that can be made again, and their files are not listed.
- Before it changes anything, the command compares the newest migration of the database with the newest migration of the binary. When the two differ, it prints `the database has the schema <found>, and this version of the server needs the schema <needed>` on standard error, changes nothing and ends with the exit code 1. The statements of the command are written for the schema of its own version.

`junto-server schema-version`:

- Prints one line on standard output, `schema <number>`, and ends with the exit code 0. The number is the newest migration that the binary applies, in decimal digits without leading zeros. The command reads no setting and no database.
- A server that runs has applied every migration of its binary, so this number is the schema of the database that a backup of that server copies. The program that makes a backup stores the line with the backup, and the program that restores compares it with the line of the installed binary before it replaces anything.

`junto-server unnamed-files [--remove]`:

- Examines the directories of `JUNTO_ATTACHMENTS_DIR`, `JUNTO_RECORDINGS_DIR` and `JUNTO_EXPORTS_DIR` that are set. It is run with the settings of the server.
- A directory is unnamed when its name is an identifier in the form that the server writes, it is a directory and not a link or a file, and no row names it: no attachment has the identifier as its storage, no recording has it and no queued removal names it, no export has it. No other entry is examined. Maintenance sweeps `rescued/` (ADR 0606).
- Without `--remove` the command prints `unnamed <kind>/<identifier>` on standard output for each unnamed directory and changes nothing. With `--remove` it deletes each one and prints `removed <kind>/<identifier>`. The kind is `attachments`, `recordings` or `exports`. The exit code is 0, also when nothing was found. When a directory cannot be deleted, the command goes on with the others, and ends with the exit code 1 and the first such failure on standard error.
- An unnamed directory whose last change is less than 24 hours ago stays, and the command prints `skip <kind>/<identifier> changed_recently`. The last change is the time at which an entry was added to the directory or removed from it.
- The server writes the row of an upload, of a recording and of an export in a transaction that has committed before the directory is created, and it deletes a directory before it deletes the last row that names it. The command reads the names of a directory first and the rows afterwards, so a directory that the server writes is named by a row that the command reads. The 24 hours are a second condition that does not depend on this order.
- The work is one full scan of each directory: the command reads every name, keeps the identifiers in memory, 16 bytes each, and reads the rows for 1,000 identifiers in one transaction.
- With `--remove`, each transaction that reads rows also reads whether a backup holds deletions back. When one does, the command deletes nothing further and ends with the exit code 1.
- A deletion starts from the configured directory, deletes the entry of the examined name only, and follows no link (ADR 0606).
- The command changes nothing and ends with the exit code 1 when none of the three settings is given, when two of them name the same directory, and when the newest migration of the database differs from the newest migration of the binary. A database of another schema can name its directories in a way that this binary does not read.

Copy before a migration:

- Before the server connects, it reads whether this version would apply a migration to a database that has a schema. If so, it copies the database to `<name>_before_<newest applied migration>` on the same PostgreSQL server, with `CREATE DATABASE ... TEMPLATE`, and logs the name.
- The copy has a comment that marks it and names the database that it is a copy of: `junto: copy of the database <name> before a migration`. The programs of a deployment find a copy by this comment. A database of another server whose name starts in the same way, and a copy of it, stay.
- When the copy exists, and before the first migration, the server writes a line at the start of the comment of the database itself: `junto: an upgrade of this database has not finished. <copy> is the copy from before the upgrade`. A comment that the database had stays after that line. Each start removes the line after its migrations ran to their end, also when none waited.
- The server knows by this line that an upgrade did not finish. A start that finds it makes no copy, deletes no copy, and applies the migrations that wait. A migration that fails leaves the migrations before it applied, so the database of such a start can be migrated in part, and the earlier version does not start on a copy of it. This holds also when the copy from before the upgrade is no longer there.
- The state is in a comment and not in a table, because the copy is made before the migration that would create such a table. It is on the database and not on the copy, so that a copy has the same comment in both states and the programs of a deployment find it by that one comment; after a way back that renames the databases, the line is on the database that was migrated in part. The server writes the state and does not derive it from the numbers of the migrations, because a version with fewer migrations than the failed one can finish the upgrade.
- A start that makes a copy first deletes the copy of the upgrade before, so the server keeps one copy.
- Before the copy is created, the comment of the database gets the first line `junto: the server makes the copy <name>_making_<newest applied migration>`. The copy is then created under that name, then gets its comment, then gets the name of a copy, and then the database gets its line of an unfinished upgrade in place of the first one. A stop of the server between these steps leaves no database that has the name of a copy and not its comment. After a stop before the last step, the next start replaces the copy; no migration has run at that time.
- A start that would make a copy first deletes what a stopped start left: a database `<name>_making_...` with the comment of a copy of this database, and a database without any comment whose name the comment of the database of the server states as the copy that the server makes. The second one is the result of a stop before the comment of the copy was written. PostgreSQL creates no database with a comment, so the server records its intent on its own database first; a name alone does not show who made a database.
- Every other database of such a name stays. When one of them has the name under which this start creates its copy, the start stops with a message that names it: the server did not record that it made that database, so it does not delete it.
- A database that has the name of the new copy and not the comment of a copy of this database is not a copy that this server made. It stays, and the server does not start.
- PostgreSQL cuts a database name after 63 bytes. When the name of the copy would be longer, no copy is made and the server does not start.
- When the copy cannot be made, the server does not start and migrates nothing. PostgreSQL copies only a database that no other program is connected to, and the database user must be allowed to create a database.
- While a recording is starting or runs, no copy is made and the server does not start (ADR 0608).
- `JUNTO_MIGRATION_COPY=none` turns the copy off. The default is `database`.
- The first start on an empty database copies nothing.

Newer schema:

- A database that holds a migration that this version does not know stops the start with an error that names the migration.

## Consequences

- A backup needs no stop of the server. The script of the deployment calls `backup-begin`, copies the database, calls `backup-files`, copies the directories of its `file` lines, and calls `backup-end`. When `backup-end` fails, the backup did not hold deletions back until its end, and the script discards it.
- The way back after a failed upgrade is: stop the server, rename the database, rename the copy to the name of the database, start the earlier version.
- The copy doubles the disk space of the database until the upgrade after a finished one replaces it. A server that is started again after a failed migration makes no further copy.
- The comment of a database whose upgrade did not finish states that and names the copy; a list of the databases with their comments shows it.
- When the copy of an unfinished upgrade was deleted, the upgrade is finished without a copy.
- A directory that no row names stays on the disk until `unnamed-files --remove` runs at least 24 hours after its last change. After a restore of a recent backup, and after the way back from an upgrade, a first run lists such directories as `skip` lines.
- A backup that never reports its end holds deletions back for 24 hours at most.
- A restored database can name an upload or a recording that was not complete when the copy was made. The upload expires, and the controller of recordings ends a recording without a recorder (ADR 0604).
- A second server process on the same database prevents the copy, and with it the migration. An upgrade stops every server process first.
- The server has no command that writes or reads a backup file.
