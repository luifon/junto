# 0022. The database role of the server and the access rules of PostgreSQL

## Status

Accepted

The role, the access rules and the step for an existing deployment were run in the local form, and the first two also in the one-host and the two-host form on one machine. On 2026-10-16 the role and access rules were used by the running two-host Linux deployment and its successful backup and isolated restore (ADR 0021). Open: the existing-deployment conversion on public hosts. The scripts of a checkout from before this decision were not run on a deployment that has the role, except the server of that checkout and `deploy/upgrade-undo.sh`.

## Context

The Compose files created PostgreSQL with one user, `junto`, and the server connected as that user. The PostgreSQL image makes the user of `POSTGRES_USER` the first user of the data directory. That user is a superuser: it reads and writes every file that the PostgreSQL process can reach, runs programs in the container, and changes every database and role. The server needs none of that. For the copy before a migration it needs the right to create a database and the ownership of its own database (ADR 0018); for its migrations it needs the ownership of its tables.

PostgreSQL does not let the first user of a data directory lose the superuser attribute, and it refuses `REASSIGN OWNED` for that user. A data directory keeps its first user for its whole life, and the variables of the image act only when the directory is created. The scripts of the deployment run `psql`, `pg_dump`, `createdb` and `dropdb` inside the container of PostgreSQL, through its socket. The password of the first user is the value that the server has in its database URL. The way back after an upgrade starts the Compose file and the server of an earlier release (ADR 0021).

## Decision

Two users:

- The server connects as the role `junto_server`. The role has `LOGIN` and `CREATEDB` and no other attribute: it is no superuser, creates no role, does not replicate and does not bypass row security. It owns the database `junto` and the tables, sequences and routines in it. Its password is `JUNTO_POSTGRES_PASSWORD`.
- The first user keeps the name `junto` in a new and in an existing data directory. It is the user of the administration: the scripts of the deployment and the commands of the install document use it through the socket of the container. One name in every deployment keeps the scripts and the documents the same for both.
- The role of the server has a name of its own, so a server of this decision does not start on a deployment that was not brought over: it cannot log in. A server that kept the name `junto` would connect as the superuser without a message.

Access rules:

- `deploy/images/postgres/pg_hba.conf` is mounted into the container, and PostgreSQL starts with `-c hba_file`. It has two rules: every role connects through the socket without a password, and `junto_server` connects over the network with its password. No rule names another role, so the first user has no path over the network, also from the loopback address of the container.
- The socket is in a directory of the container that no other container mounts.
- The rule is in the Compose file and not in the data directory. The password of the first user stays as it is. The Compose file of an earlier release starts PostgreSQL with the rules of the data directory, and its server logs in as `junto`.
- A change of the secret in a release of this decision changes the password of `junto_server` only, and a restore gives only that role the password of the backup. `deploy/upgrade-undo.sh` therefore gives the role that the server of its checkout connects as the password of the settings file, before it runs that server on the copy. For a release before this decision that role is the first user.
- A removed password of the first user was not chosen: it is a state of the data directory, the way back would have to set it again, and a later upgrade would have to remove it again.

What the first user reads:

- The owner of a database can create an object with the name of a system object in it, and can set the search path of the database. A statement that the first user runs there with a name without a schema can so run a function of the role with every right.
- The first user connects to the database `postgres` for the administration: the scripts, and the commands of the install document, name it. The role of the server owns nothing there and may create nothing in its schema.
- The one program that the first user runs in the database of the server is the role file. Its first statement sets the search path to the system schema, so every name that it reads is a system name, and the names that it writes into its statements carry their schema.
- `pg_dump` and `pg_restore` run as the role of the server, not as the first user.

The role file:

- `deploy/images/postgres/server-role.sql` is run by `psql` as the first user, in the database that the role is to own. It creates the role when it is absent, sets its six attributes and its password, makes it the owner of the database, and makes it the owner of each table, view, sequence, routine, type and schema that the first user owns in that database and that is not part of PostgreSQL or of an extension. It changes each object, because `REASSIGN OWNED` is refused. Indexes and the sequences of columns follow their tables.
- The file reads the password from the variable `POSTGRES_PASSWORD` of the container, so the password is in no command line. The statement that sets it holds the password as text. For that statement the file turns off the three settings by which PostgreSQL writes a statement to its log, and the context lines of `psql` for a statement that fails. `deploy/upgrade-undo.sh` does the same for its statement.
- A second run changes only what differs. A membership in another role and a privilege that somebody granted to `junto_server` stay: the file sets the attributes of the role and the owners, and it does not read what an operator added.
- The object kinds are the ones that own data or code in a schema. The migrations of the server create tables, sequences, routines and indexes. An object of another kind that the first user owns keeps its owner, and a migration that changes it then fails with the message of PostgreSQL that names it.

Where the file runs:

- A new data directory: the file is mounted as an init file of the PostgreSQL image, which runs it once in the database `junto` after it created the directory.
- A deployment of an earlier release: `deploy/database-user.sh <form file>` runs it in the database `junto` and in each copy from before a migration that has the comment of the server (ADR 0018), each in one transaction. It refuses while the container of the server runs or is being started again, and it starts PostgreSQL with the Compose file of its checkout first.
- A restore: `deploy/restore.sh` runs it in the staged database before the dump is loaded, and the role loads the dump with `pg_restore --no-owner`. A restore so needs no step for the role, and the restored objects belong to the role whatever role owned them in the backup.

What the scripts do as which user:

- `pg_dump` and `pg_restore` run as `junto_server`: a backup and a restore need no right that the role does not have.
- The creation, the deletion and the renaming of a database, and the selection of the copies by their comments, run as the first user. A deletion with `--force` ends the sessions of other roles, which the role of the server cannot.
- The commands of the server binary (`backup-begin`, `backup-files`, `backup-end`, `restored`, `schema-version`, `unnamed-files`, `new-event-epoch`) run with the settings of the server, so as `junto_server`.

## Consequences

- A statement of the server cannot read a file of the database host, run a program there, or change a role, while the operator gave the role no membership and no further right. It can create a database, and delete and rename the databases that it owns.
- A `psql` session of the first user in the database `junto` that a person opens by hand, with statements that name no schema, is outside this rule.
- Every process inside the container of PostgreSQL is the first user. A person who can run `docker compose exec postgres` has every right in the database server, as before.
- An upgrade from a release before this decision has one more step, `deploy/database-user.sh`. Without it the server does not start, and its log states `password authentication failed for user "junto_server"`. PostgreSQL gives the same message for an absent role and for a wrong password.
- The copy before a migration belongs to the role, because the role creates it. A copy that the first user made in an earlier release is given to the role by the step, so that the next upgrade can delete it.
- After a way back to a release before this decision, the server of that release runs as the first user on a database that the role owns. A table that such a release creates belongs to the first user, and a later migration of the role fails with `must be owner of`. A second run of `deploy/database-user.sh` corrects that.
- The database `junto_undone` that a way back keeps is not changed by the step. It is deleted by the first user.
- The two files are read by the user 70 of the container from the checkout. A checkout whose files other users cannot read does not start PostgreSQL; its log names the file.
- A change of the PostgreSQL image that needs another rule, for example for replication, is a change of `pg_hba.conf` in the repository.
- `deploy/tests/smoke.sh` checks the attributes of the role, the owners, the user of the sessions of the server, the access file that PostgreSQL reads, a login of the role over the network, and that the first user is refused there. `deploy/tests/upgrade.sh` checks the step, also against a view of the role with the name of a system view, and the way back with another password of the first user (ADR 0911). `deploy/tests/check_rendered.py` checks the Compose files and the rules of the access file.
