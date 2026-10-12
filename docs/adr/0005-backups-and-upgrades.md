# 0005. Backups and upgrades

## Status

Accepted

## Context

The data of a server is in two places, the PostgreSQL database and the files on disk, and a backup is usable only when the two match. Files can be deleted or still be written while a backup runs. A database migration that fails, or that runs twice at the same time, can damage the data. A migration that runs during a recording changes the schema under a job that is writing.

## Decision

Backups:

- Backups are the administrator's responsibility. The server makes them consistent.
- The server provides a backup command and a restore command. Both cover the database, the files, the configuration and the keys.
- A backup lists the files it needs. The server keeps those files until the backup reports that it finished.
- Files that are still being written, such as the files of a running recording, are left out of the backup and are reported.
- The restore procedure is part of the test plan (ADR 0902).
- ADR 0018 states what the server binary provides of these commands and what the programs of the deployment do. ADR 0021 states the scripts of the deployment.

Upgrades:

- Database migrations go forward only.
- The server copies the database before a migration.
- The server takes a lock so that only one migration runs.
- The server does not migrate while a recording runs. It refuses to start instead, and a host command tells beforehand whether a recording runs (ADR 0608).
- The recorder container is not stopped, restarted or upgraded while a recording runs. A restart of the recorder loses the recordings in progress, and the recorder does not resume them (ADR 0601).
- The server refuses to start on a schema that is newer than the schema it knows.
- The server connects to its database as a role that owns the database and may create one, which the copy before a migration needs, and that has no further right in the database server (ADR 0022).
- The server is never updated automatically (ADR 0704).

## Consequences

- The administrator schedules the backups and stores them.
- A file that is deleted while a backup runs stays on disk until the backup reports that it finished.
- A recording that runs during a backup is not in that backup.
- A backup is restored with the version of the server that wrote it, and is upgraded afterwards (ADR 0018, ADR 0021).
- An upgrade that contains a migration, or that replaces the recorder, waits until no recording runs.
- There is no migration to an earlier schema. An older server version does not start on a database that a newer version migrated. The way back is the copy from before the migration, or a backup (ADR 0021).
