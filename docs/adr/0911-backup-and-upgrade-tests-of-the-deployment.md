# 0911. Backup and upgrade tests of the deployment

## Status

Accepted

Open: both tests run the local form on one machine. They were run on Docker Desktop on arm64 and on no other system, and never on a CI runner.

## Context

ADR 0902 decides that a restore from a backup is tested. ADR 0021 decides a backup of a running server, a restore whose replacement can be interrupted, and a way back after an upgrade. ADR 0022 decides the database role of the server and a step that gives it the database of an earlier release. A test of these needs an order of events that does not occur by chance: a deletion between the copy of the database and the list of the files of one backup, and an end of the restore script between two of its steps. The server deletes the directory of a removed attachment at the first maintenance pass that runs at least 60 seconds after the removal, and a pass runs every 60 seconds. The scripts under test call one program, `docker`. The earlier test of the backup wrote files into the volumes by hand, which a backup that copies the listed directories does not hold. A recording through the recorder takes 9 minutes and 13 GiB of free disk (ADR 0910).

## Decision

Both tests:

- They start the database and the server of the local form only, with new secrets, and remove the containers, the volumes and the network at the end.
- `deploy/tests/content_driver.py` creates the content through the API as a client does, with `deploy/tests/junto_api.py`: an owner, channels, messages with one attachment each.
- A recording of these tests is a row of the state `ready` that the driver writes into the database, and a directory with two files. The removal of such a recording goes through the API. A backup copies the directory of a recording whatever wrote it; the recorder is covered by ADR 0910.
- The scripts under test have no option for a test. Where a test needs a step between two calls of a script, it puts a program named `docker` first in the `PATH` of that script. The program runs the step before or after the one call whose arguments hold a stated text, and passes every call on to Docker.
- A wait for a deletion by the server is 150 seconds: the 120 seconds that the deletion of an attachment directory takes at most, and 30 more.

`deploy/tests/backup-restore.sh`:

- A missing release image is refused with its preparation command before the running
  server is stopped, and no image is built. The empty restore deployment uses a second
  Compose project name and the prebuilt images of the first; neither a server nor an init
  image is created under the second project's name.

- Before the backup one channel and one recording are deleted, and the test waits until their directories have left the disk.
- During the backup a process sends messages with attachments to a channel that stays. The backup starts when three of them are stored.
- Before the call of `backup-files`, and so after the copy of the database, the test deletes a second channel and removes a second recording, waits 150 seconds, and records that their directories are on the disk. After the backup it waits until the server has deleted them. The first wait fails the later checks when the backup does not hold deletions back; the second shows that the server does delete such directories in that time.
- Checks of the backup: its six files, the file `schema` with the newest migration of the database, no access for the group and for others, no value of a secret in the output of the script, the server container was not stopped, the process that wrote messages ended without a failure, the archive holds the directories of the kept content and of what was deleted after the copy, and none of what was deleted before, and `left-out` has no line for the recording that the driver wrote as failed and without a directory.
- Refused restores: an archive that was cut, an archive with other member names, an archive with a link, a list of checksums that names a file with a similar name and not the archive, a file `schema` with the number above and with the number below the one of the server, and a list of checksums without the file `schema`. For the two schemas the test checks that no container was stopped. A copy of the database that was cut fails while it is staged; the test checks that the server and the files are as they were and that no staged part is left.
- The new deployment gets an export, a directory in the spool and a file in the temporary directory of the recorder before the restore; the test checks that the restore deleted them.
- The restore into a new deployment with other secrets is ended with `SIGKILL` after the files were replaced and before the database has its name. The test checks that a server that is started then does not become healthy and that its log names the absent database, then runs the restore again.
- Checks of the restored deployment: the server has the identifier of the backup; the role of the server owns the database and every table, sequence and routine in it; the kept channel and the channel that was deleted after the copy hold their messages; the messages that were written during the backup are a first part of the messages that were sent, and include every message from before the backup; every attachment of every message is returned with the bytes of its checksum; the channel that was deleted before the backup is not returned and none of its directories is on the disk; every directory of a complete attachment that the database names is on the disk; the two recordings that the database names have the files of the backup; no backup holds deletions back; the directory of the exports is empty; the server takes a new message.
- The directories without a row: the test computes them from the members of the archive and the rows of the restored database, and compares them with the lines that the restore printed. The process that writes messages during the 150 seconds between the copy of the database and the list makes such directories. The test also checks that a restored directory has the time of its last change from before the backup.
- Two further backups of the restored deployment. In the first, the test moves the end of the backup into the past after the archive was written; `backup-end` must fail, and the script must fail and leave no directory. Before the second, the test gives the directories without a row a last change of two days ago, and deletes the directory of one complete attachment and of one finished recording; the script must remove and print exactly the directories without a row, write the backup, name the two lost directories in `left-out` and no other, and end with the code 4.

`deploy/tests/upgrade.sh`:

- The earlier release is the commit of the deployment before the server copied its database. Its files come from `git archive` into the directory of the test, and its Compose file starts it under the same project name, so the volumes are the ones of the later start.
- The server of the earlier release connects as the first user of PostgreSQL. A server of this release on that deployment: the log names the role that cannot log in, nothing is migrated.
- `deploy/database-user.sh`: it is refused while the container of the server is started again and again; then the role has its two attributes and no other, and owns the database and every table, sequence and routine; a second run ends without a failure; the output holds no secret.
- The step against a name of the role: the role creates a view with the name of the system view of the roles, whose function records the user that runs it, and sets the search path of the database so that the view is found first. The step runs again, and nothing is recorded.
- A backup while the server is stopped, its image is of this release and the database has the schema of the earlier release: the script names both schemas, makes no backup and leaves no directory.
- A role without the right to create a database, which the test takes away: the log names the cause, nothing is migrated, no copy exists. A further run of the step gives the right back.
- An upgrade that fails in its second migration, because a function of that migration exists: the log names the failure, the schema is between the two releases, the copy from before the first migration exists and belongs to the role, and the comment of the database names it. A second start fails in the same migration and leaves the schema, the copy and the schema of the copy as they were. The container of these starts has no restart rule, so that the stop of the test cannot end a start that is making the copy.
- Before the way back the test gives the first user another password than the one of the settings, as a change of the secret in this release leaves it. The server of the earlier release connects as that user, so the way back and the start that follows work only when the script gives the user the password of the settings.
- Then `deploy/upgrade-undo.sh` without a name: no copy is left, the database has the earlier schema and no comment, the event stream has another epoch, `junto_undone` has the comment of the unfinished upgrade, the script states that the earlier release cannot list the directories without a row, and the earlier release serves the content again.
- The earlier commit is older than `deploy/upgrade-undo.sh`. The test copies the script into the files of the earlier commit and runs it there, as a later release is used.
- The upgrade: every migration, the copy, the name of the copy in the log, no comment of an unfinished upgrade, sessions of the role of the server, the role as the owner of every object of the new schema, the health check `junto-server health`, a healthy server, a writable directory of the exports, the same content.
- The way back after an upgrade that worked, with a message that was written after the upgrade: the script refuses while the `junto_undone` of the first way back exists and changes no database; after its deletion, the earlier release serves the content without that message. Then the upgrade once more.
- A backup as the earlier release wrote it, which has no file `schema`, is refused by `deploy/restore.sh` with the message of the server that names both schemas, and nothing is replaced.
- A backup with the server stopped, as the upgrade procedure makes it, then a further message, then the restore of that backup: the copy from before the migration is deleted, and the server serves the content without that message.
- The restore while the server of this release runs and the image of the service is the one of the earlier release, which states no schema: the restore is refused and stops nothing.
- Two databases with the name of a copy that is being made, one that the comment of the database names and one that nothing names: the restore deletes the first and leaves the second, and the way back names the first and not the second.
- A way back inside this release. No later release exists, so the test creates a copy as the server does, with its name and its comment, and sends a message afterwards. `deploy/upgrade-undo.sh` of this checkout takes the copy and prints the directory of that message as a `skip` line. `junto-server unnamed-files --remove` leaves the directory, the test gives it a last change of two days ago, and the command removes it.

Continuous integration:

- Both tests run in the job of the deployment (ADR 0905). The job fetches the whole history for `git archive`.
- `deploy/tests/test_restore_images.py` runs the restore with real Compose rendering and
  simulated container commands. Missing server/init, PostgreSQL and recorder images, and
  a missing explicit image setting, must fail before any container command. The check
  also covers backups without a schema file, selection by image ID, absence of builds
  and pulls, and build definitions removed in all data-bearing deployment forms.

## Consequences

- The backup test takes about 8 minutes, of which up to 7.5 are waits for deletions.
- The program in front of `docker` selects a call by a text of its arguments: `backup-files`, the option `--verbatim-files-from` of the archive step, and the path of the exports in the replacement step of the restore. A change of those calls needs a change of the test; the test fails when the step did not run, because the checks of the step fail.
- A change of the columns of `recordings` needs a change of the row that the driver writes.
- The upgrade test builds the server image of the earlier release. Both releases build an image of the same name, so every start of the test names its build, and a build that was made before comes from the build cache.
- The upgrade test names a function of the second waiting migration. Another earlier commit (`JUNTO_UPGRADE_FROM`) needs another name there.
- The tests do not cover a recording that runs during a backup, a transcription that runs during a backup, a way back to a later release that a real upgrade left, a stop of the server while it makes its copy, a restore that is ended by force while it unpacks the archive, and the one-host and two-host forms.
- A failure of a test prints the last lines of the log of the server, which can hold the owner code of the deployment of the test. That deployment is removed at the end.
- The limits of 150 seconds follow two values of the server. A change of the maintenance interval or of the wait before an attachment directory is deleted needs a change of the driver.
