# 0009. Serializable transactions with a bounded retry

## Status

Accepted

## Context

A request reads state and then relies on it: the session and the state of its account, the roles and overrides that decide a permission, a count that a ceiling limits, the row that it then changes. With the default isolation level of PostgreSQL, each statement sees the changes that other requests committed in between. A review found the results: an invite created by an account while it was banned, a session that survived a ban, a permission restored after it was revoked, a private channel shown during its deletion, a ceiling exceeded, and a field overwritten by a concurrent edit. The server needs one rule that covers these cases and the ones that later features add.

## Decision

Isolation:

- Every transaction of the server runs at the isolation level SERIALIZABLE. The server opens its connections with this level as the default and names it again when it starts a transaction.
- All database work of one request is one transaction: the check of the session and of the active state of its account, the roles and overrides that the permission function needs, the reads, the writes and the audit entry.
- A request that carries a signed challenge marks the challenge as used in the same transaction (ADR 0108). A transaction that does not commit leaves the challenge unused.
- Attachment chunk uploads use the durable-intent exception of ADR 0408: one transaction authorizes an intent, immutable file I/O follows, and a second transaction checks the session and permissions again before accepting the bytes. File I/O never runs inside a retried transaction.
- A route that only reads uses a read-only transaction. All of its statements see one snapshot.
- The server code reaches the database only through two macros, one for a transaction that writes and one for a read-only transaction.

Retry:

- A transaction that PostgreSQL ends with a serialization failure (SQLSTATE 40001) or a detected deadlock (40P01) is rolled back and run again from its start.
- A transaction runs at most 8 times. Before each further attempt the server waits a random time; the upper bound of the wait starts at 8 milliseconds, doubles with each attempt and stays at 256 milliseconds.
- After the eighth loss the request gets the error `busy` with the status 503 and `Retry-After: 1`. Nothing was changed, so the client sends the same request again, a signed request included.
- The work inside a transaction has no effect outside the database, so running it again is safe. Random codes and identifiers are generated before the transaction.

Locks:

- The correctness of a request does not depend on the order in which it takes locks, and the server defines no lock order.
- Some statements take a row lock with `for update`. The lock makes the loser of a conflict wait and fail at that statement, not at its commit. A deadlock between two such locks is one of the two retried cases.

Time limits in the database:

- The HTTP time limit of 30 seconds (ADR 0008) ends the request, but it does not stop a statement that PostgreSQL is running or a wait for a lock. Without a limit in the database, each such request would keep a connection of the pool busy after its response, and a few of them would stop the server for every user.
- Every connection of the pool is opened with three limits that PostgreSQL enforces: a statement runs at most 10 seconds, a wait for a lock lasts at most 5 seconds, and a connection stays idle inside a transaction at most 15 seconds. Each is shorter than the HTTP time limit.
- A statement that PostgreSQL ends for one of these limits gives the request the error `timeout` with the status 503. It is not run again.
- The server rolls a failed transaction back before it sends the response, so the connection is free when the client reads the error.
- A request that the HTTP time limit ends while a statement runs leaves that statement to the database limits: it ends within 10 seconds, and the connection returns to the pool.
- The migrations run on a connection of their own without these limits.
- Each attempt of a transaction waits at most 10 seconds for a free connection of the pool. The wait includes the check that the pool makes on an idle connection and the opening of a new one, so it also ends when PostgreSQL stops answering for a while. A request whose attempt gets no connection in this time gets the error `busy` with the status 503 and `Retry-After: 1`: no transaction began and nothing was changed, so the client sends the same request again. The server logs a warning. The server does not run the request again, because another wait for the pool would hold the request longer than its time limit allows.

Other writers:

- The start of the server, the host commands (ADR 0114) and the maintenance task use the same macros.
- Expired rows are removed by a maintenance task once per minute, not by the requests, so that a request reads and writes only the rows of its own subject. The task works in batches of at most 500 rows. Each batch is one transaction under the same limits, so the task never holds locks for long.
- The task has several steps. A step that fails is logged with its name, and the later steps of the same pass still run, so a failure of one step does not stop the removal of other rows or the release of reserved disk space.

Test hook:

- The database object has a gate that can hold one transaction between its work and its commit. Only tests arm it (ADR 0904).

## Consequences

- The result of concurrent requests is the result of running them one after the other in some order. A ban and an invite creation, a login, or a role change by the banned account cannot both take effect in an order that no sequence allows.
- A new route gets this protection by using the macros. It needs no decision about locks.
- A request can run its database work several times. Under many concurrent writes, PostgreSQL also ends transactions that did not truly conflict, and those are retried as well.
- A client must be prepared for `busy` and send the request again.
- PostgreSQL detects conflicts only between serializable transactions. A program that writes to the database with another isolation level is outside this rule.
- No transaction of a request holds a lock or a connection for longer than the database limits allow. A request that waits behind a lock for 5 seconds fails with `timeout`, also when it would have succeeded a moment later.
- A server whose pool stays full, or whose database does not answer, answers `busy` and not an internal error, so a client can tell this state from a defect of the server.
- A statement that needs more than 10 seconds cannot be part of a request. Later features with such work run it in batches, as the maintenance task does.
