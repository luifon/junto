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
- Fixtures whose bulk preparation exceeds the normal statement time limit are inserted in batches of at most 500 rows under the normal statement time limit. The fixture size and the assertions of the query under test stay unchanged; preparing thousands of rows is not one production request.
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
- Every live test has a time limit. The body of each async live test of the server, the client core and the media engine runs under `limited` of `crates/client-core/tests/e2e/limit.rs`, which the test binaries include. When the body has not ended after 300 seconds, the test writes its failure with the last steps that it noted to the standard error of the process, past the capture of the test runner, then drops the body, which ends its servers and calls and releases what it holds, and then fails. The cleanup has a limit of 60 seconds: a drop that waits longer, for example for a database server that does not answer, ends the test process with exit code 124 and a report. A test that is not one future, such as the tests of virtual sound devices and of TURN over TLS on Linux, which wait for processes and read their pipes, holds a `watch` with the same limit, which ends the test process with the same report. The wait helpers of every live tier note their steps. A test of the window is not one future: each of its waits on the runtime of the test (`World::run`) has the limit, and its waits for the window have their own limits already. A step that takes longer by design, the writing of the long channel of the native targets, names its own limit.
- The deletion of the database of a server test has its own limit of 30 seconds.
- The tests of calls share the media server through a fair lock, so one test that waits while it holds the lock also stops every test that waits for the lock after it. A test that waits for the lock notes the tests that hold it. A test that holds the media server alone pauses or stops it; when it releases the media server, by its last step, a failure or its limit, the release unpauses and starts the server and waits until it answers, so the tests after it have a media server. A run of these tests once did not end for hours; the test that held the lock was not found.
- Every test server of LiveKit has the settings of the deployment that do not depend on the host: `room.departure_timeout` (ADR 0020) and the setting of the camera layers (ADR 0609). They are in the one file `deploy/livekit-common.yaml`. `tools/with-test-livekit.sh`, which the server, media, window and layout tiers use, and `tools/linux/run.sh`, which starts the LiveKit server of the Linux container, add the file to their configuration, with no setting that turns it off. The settings that depend on the host stay in each script: the ports, the keys, the address of the notifications, the address that LiveKit announces, and development mode. A Compose form cannot read a second configuration file into `LIVEKIT_CONFIG`, so every form repeats the file; `deploy/tests/check_rendered.py` fails when the `room` or `video` section of a form differs from the file, or when a form has a section that is neither in the file nor one of the sections of the host (`port`, `bind_addresses`, `keys`, `redis`, `rtc`, `turn`, `webhook`, `logging`). The unit tests of the wrapper, of the Linux runner and of the comparison check each part.
- The tests of recordings use a recorder in memory that behaves as step 0 measured the recorder (ADR 0604) and writes the files of its jobs. The files are generated with `ffmpeg`, so that the server reads, joins and checks real media with the programs of the product (ADR 0606). A test runs each pass of the controller, each attempt of the processing and each transcription job itself.
- The transcription worker of these tests is a stand-in script that the server runs as it runs the worker. It keeps the job file that it got and writes the transcript that the test prepared, so a test states both sides of the contract (ADR 0611). One more tier runs the worker of `workers/transcription` with its development model on generated speech; it is ignored in the plain run and fails with a message when the worker is not installed.

## Consequences

- The tests need Docker, and the tests of recordings need `ffmpeg` and `ffprobe` with the encoders `libopus` and `libx264`.
- A rule that depends on a lock or a constraint is tested with the lock or the constraint.
- A test of a time limit takes no real time.
- A machine under load makes a test slower and does not change its result: no preparation step runs under a shortened time limit.
- A live test runs against the media server behaviour of the deployment: a room stays 60 seconds after its last person left, and a camera layer above the lowest is forwarded again after two seconds with packets. A new setting of LiveKit that changes media behaviour goes into `deploy/livekit-common.yaml`; a form that has it alone fails the check of the deployment.
- CI on Windows and macOS (ADR 0902) needs a PostgreSQL that those runners can start; the script covers Linux and macOS.
