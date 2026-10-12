# 0018. What the server binary does for a backup, after a restore and before a migration

## Status

Accepted

## Context

ADR 0005 states that the server provides a backup command and a restore command, that a backup lists the files it needs and the server keeps them until the backup reports its end, that files that are still written are left out and reported, and that the server copies the database before a migration. None of this existed. The scripts of the deployment copy the database with `pg_dump` and the files with `tar` while the server is stopped. The image of the server holds no `pg_dump`, the settings file of a deployment and the keys of the entry are outside the container of the server, and a program that copies a PostgreSQL database must have the version of that database. The server deletes files apart from its requests: the directories of removed recordings (ADR 0608) and of removed attachments (ADR 0408).

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
- A backup that has ended or expired has no list: the command ends with a failure.
- The command reads the disk to learn whether a removed directory is still there. When the database has attachments or recordings and the command does not see the directory of that kind, because the setting is absent or names no directory, it prints no list and ends with a failure.
- The task that deletes the directories of attachments reads the backup in the transaction that selects the directories, so a directory that is removed after the start of a backup is not selected by a run that began before.

`junto-server backup-end <identifier>`:

- Ends the backup. The deletions that waited are done at the next run of their tasks. An unknown identifier ends the command with a failure.

`junto-server restored`:

- Is run after the database and the files of a backup were put in place, before the server starts.
- Gives the event stream a new epoch (ADR 0010), removes every stored backup, and removes every export.
- Exports are not part of a backup: they are copies that can be made again, and their files are not listed.

Copy before a migration:

- Before the server connects, it reads whether this version would apply a migration to a database that has a schema. If so, it copies the database to `<name>_before_<newest applied migration>` on the same PostgreSQL server, with `CREATE DATABASE ... TEMPLATE`, and logs the name.
- The copy has a comment that marks it and names the database that it is a copy of. The copy of an earlier migration of the same database, recognised by that comment, is deleted first, so the server keeps one copy. A database of another server whose name starts in the same way, and a copy of it, stay.
- PostgreSQL cuts a database name after 63 bytes. When the name of the copy would be longer, no copy is made and the server does not start.
- When the copy cannot be made, the server does not start and migrates nothing. PostgreSQL copies only a database that no other program is connected to, and the database user must be allowed to create a database.
- While a recording is starting or runs, no copy is made and the server does not start (ADR 0608).
- `JUNTO_MIGRATION_COPY=none` turns the copy off. The default is `database`.
- The first start on an empty database copies nothing.

Newer schema:

- A database that holds a migration that this version does not know stops the start with an error that names the migration.

## Consequences

- A backup needs no stop of the server. The script of the deployment calls `backup-begin`, copies the database, calls `backup-files`, copies the directories of its `file` lines, and calls `backup-end` (ADR 0021).
- The list has a `file` line for every recording that is not starting, running or processed, also for one without a file. A program that copies the list accepts a listed directory that is not on the disk.
- The way back after a failed upgrade is: stop the server, rename the database, rename the copy to the name of the database, start the earlier version. `deploy/upgrade-undo.sh` does the renames (ADR 0021). The copy holds no file: a file that the newer version deleted is not brought back.
- A start that follows a failed start with a waiting migration deletes the copy and makes it again. When the failed start had applied one of several migrations, the new copy has the schema after that migration, and its name has that number.
- The copy doubles the disk space of the database until the next migration replaces it.
- A backup that never reports its end holds deletions back for 24 hours at most.
- A restored database can name an upload or a recording that was not complete when the copy was made. The upload expires, and the controller of recordings ends a recording without a recorder (ADR 0604).
- A second server process on the same database prevents the copy, and with it the migration. An upgrade stops every server process first.
- The server has no command that writes or reads a backup file.
