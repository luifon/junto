# 0010. Event positions are assigned at the commit, under one lock

## Status

Accepted

## Context

The event stream (ADR 0107, ADR 0201) needs one order of the events of a server, because a client states with one cursor what it has received. Requests run as concurrent serializable transactions (ADR 0009). A number that a transaction takes from a sequence when it writes an event does not give that order: a transaction that took the number 5 can commit after a transaction that took the number 6. A reader that has delivered event 6 would then deliver event 5 late or never.

## Decision

Positions:

- Every event has a position: a 64-bit integer that is unique in the log of the server. It is the `id` of the event on the wire (ADR 0206).
- An event with a higher position never becomes visible to a reader before an event with a lower position.
- The events of one transaction get rising positions in the order in which the transaction wrote them.
- Positions can have gaps. A gap has no meaning.

How a position is assigned:

- A transaction writes an event row without a position.
- A deferred constraint trigger of the table runs when the transaction commits. For each event row of the transaction it takes one advisory transaction lock, the same for the whole log, and then stores the next value of a sequence as the position of the row.
- PostgreSQL releases the lock of a transaction after the commit of that transaction is visible to others. The next transaction therefore gets the lock, and its positions, only after every lower position is visible.
- The rule is in the database. It holds for every writer: requests, the host commands (ADR 0114) and the maintenance task.

Why the order holds:

1. A deferred trigger runs inside the commit of its transaction, before PostgreSQL records the commit. The trigger takes the lock first and asks the sequence for a value only then. A transaction lock is kept until the end of the transaction, so the writer holds the lock from its first position to the end of its commit.
2. A transaction with several events takes the same lock again for each of them and keeps it throughout. No other writer gets a position in between. The sequence gives one value per call and keeps no values ready in a session, so the values rise in the order of the calls.
3. At a commit PostgreSQL first records the commit, then takes the transaction out of the set of running transactions, and only then releases its locks. The next writer can therefore get the lock, and a position, only after the writer before it has stopped to count as running.
4. A reader takes its snapshot under the same internal lock that guards that set. A snapshot sees a transaction either as running or as ended, never something in between. A reader that sees the later writer's events took its snapshot after that writer left the set, which is after the earlier writer left it, so the reader sees the earlier events too.
5. A serializable read-only transaction keeps one snapshot for all its statements. PostgreSQL can end it with a serialization failure, but cannot show it a later commit without the earlier one.
6. A writer that fails after it took positions, at the trigger, at a time limit or with a serialization failure, is rolled back with its events. Its positions are not given again. They stay unused; they cannot appear later behind a cursor.
7. The newest position that a reader works with is the highest position of the rows that it sees, not the state of the sequence.
- A second trigger refuses every change of an event row whose position is set, and every change of another column. Rows are removed only by the trimming of the log (ADR 0207).
- Each row also has a UUID as its key in the table. It is not sent to clients.

Epoch:

- The server stores one identifier of this history of positions, the epoch, created with the log.
- A cursor is the pair of the epoch and a position (ADR 0207). The server serves no cursor of another epoch: it answers `resync`, also to a connection that subscribed before the epoch changed.
- The host command `junto-server new-event-epoch` stores a new epoch and writes an audit entry. It works on the database only, like the commands of ADR 0114.
- After the restore of a backup (ADR 0005) the operator runs this command before the server accepts clients. The restored database holds the positions of the time of the backup, and the clients hold cursors of a history that the server no longer has.

## Consequences

- A reader that reads the events after a position in one snapshot of the database gets all of them. No event with a lower position can appear later.
- The commit steps of the transactions that write events run one after the other. The lock is held for the commit step only: a transaction that waits for another lock, or that the commit gate of the tests holds (ADR 0009), does not hold it. A transaction that writes no event is not affected.
- A commit that waits for the lock longer than the lock time limit of ADR 0009 fails with `timeout`, and nothing was changed.
- A transaction that fails after its positions were taken, for example with a serialization failure at the commit, leaves those positions unused.
- A client cannot tell from two positions how many events lie between them.
- A restore without the command leaves the old epoch. A client whose cursor is at or below the restored newest position is then served a replay that does not match its state. The restore command of ADR 0005 has to run the epoch command itself when it is built.
- Known limit: a recipient sees from the positions it receives that the server recorded other events in between. It learns at most their number, and nothing about what they concern.
- The guarantee rests on the order of the steps of a commit in PostgreSQL, as stated above, and not on the tests. Two tests show it at work: one holds a transaction before its commit while a later one commits, and one runs many writers at the same time and compares what a client received with the log. They do not force every order of two commits.
- A new epoch reaches a connection at its next read of the log, at the latest at its next heartbeat. A read that runs while the epoch changes ends with the snapshot it has.
