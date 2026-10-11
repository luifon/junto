# 0904. Server tests against PostgreSQL

## Status

Accepted

## Context

The server builds its SQL statements at run time (ADR 0008) and relies on constraints, transactions and row locks of PostgreSQL for several rules: atomic counting of invite uses, single use of a challenge, one owner. A test with a replaced database layer would not exercise them. Expiry rules depend on the time, and a test that waits for real time is slow.

## Decision

- The integration tests of the server start the real router on a local port and call it over HTTP.
- They run against a real PostgreSQL. The script `tools/with-test-postgres.sh` starts one in Docker on a free local port, passes its address in `JUNTO_TEST_DATABASE_URL`, runs a command, and removes the container with its volume. The container has the name `junto-dev-postgres`, or the name in `JUNTO_TEST_POSTGRES_CONTAINER`, so that two checkouts on one machine can run their tests at the same time. It is the one script for a temporary PostgreSQL; the tests of the client use it too.
- Each test creates its own database and applies the migrations to it, so tests run in parallel and do not share state.
- A test without `JUNTO_TEST_DATABASE_URL` fails with a message that names the script. It is not skipped.
- A test replaces the clock (ADR 0008) and moves the time forward to test expiry.
- A test that shortens a time limit applies it only to the step that it examines, and prepares under the normal limits. For the time limit of a request and the time limits of the database, the test prepares on one server process and sends the examined step to a second process on the same database that has the short limit. For the time limits of the event stream, the test replaces the limits of the running server after its preparation. Only tests replace limits at run time.
- A test waits for a condition and not for a fixed time when it needs something to have happened. For the admission check, the server states whether a check runs or a room waits for one.
- The permission function has table tests for each rule and property tests for the rules that must hold for every combination of roles and overrides.
- A test compares the committed OpenAPI document with the generated one (ADR 0205).
- A test checks that the database itself refuses states that break the invariants.
- A test of a race makes the order of the two requests fixed: it arms the commit gate (ADR 0009) to hold the first request before its commit, starts the second request, waits until PostgreSQL reports a statement that waits for a lock where the case needs it, and then releases the gate.
- One table lists every operation that needs a session, with a caller who lacks the permission and the expected denial. For each row the test checks the response and that no row of any table changed. The table is compared with the operations of the generated OpenAPI document, so a new operation that is not in the table fails a test.
- A test compares the complete responses for an absent object and for an object that the caller may not see.
- The host commands (ADR 0114) are tested by running the server binary.
- On a machine where other work compiles, heavy commands run through `tools/serial.sh`.
- The tests of calls have two tiers. The first tier is part of the integration tests: the server gets a media server in memory that behaves as step 0 measured LiveKit (ADR 0508), the tasks of calls do not run by themselves, and a test runs each admission check itself, so that the order of things is fixed.
- The second tier runs against a real LiveKit server. The script `tools/with-test-livekit.sh` starts one in Docker in development mode with the name `junto-dev-livekit`, runs a command, and removes the container. Real participants are processes of `spikes/test-participant` (ADR 0902). These tests are marked as ignored, so the plain run does not need LiveKit; they wait for the media server with bounded polling.
- The tests of recordings use a recorder in memory that behaves as step 0 measured the recorder (ADR 0604) and writes the files of its jobs. The files are generated with `ffmpeg`, so that the server reads, joins and checks real media with the programs of the product (ADR 0606). A test runs each pass of the controller, each attempt of the processing and each transcription job itself.
- The transcription worker of these tests is a stand-in script that the server runs as it runs the worker. It keeps the job file that it got and writes the transcript that the test prepared, so a test states both sides of the contract (ADR 0611). One more tier runs the worker of `workers/transcription` with its development model on generated speech; it is ignored in the plain run and fails with a message when the worker is not installed.

## Consequences

- The tests need Docker, and the tests of recordings need `ffmpeg` and `ffprobe` with the encoders `libopus` and `libx264`.
- A rule that depends on a lock or a constraint is tested with the lock or the constraint.
- A test of a time limit takes no real time.
- A machine under load makes a test slower and does not change its result: no preparation step runs under a shortened time limit.
- CI on Windows and macOS (ADR 0902) needs a PostgreSQL that those runners can start; the script covers Linux and macOS.
