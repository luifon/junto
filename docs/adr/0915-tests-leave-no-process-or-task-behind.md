# 0915. A test leaves no process and no scheduled task behind

## Status

Accepted

## Context

The tests of the client library, the window tests and the native targets of the client start the server binary of the checkout as a process of its own, through the harness in `crates/client-core/tests/e2e/support.rs`. Each server uses a database of the PostgreSQL that `tools/with-test-postgres.sh` starts, which Docker publishes on a free port of the loopback address. The harness stopped a server only in `Drop`. A process of the tests that a signal ends (SIGKILL, SIGTERM, the end of its terminal) runs no `Drop`, so its servers kept running. The wrapper then removed the PostgreSQL container, and each such server tried to connect to the freed port at each poll of its event log, about 25 times per second. Eleven such servers, from test runs one to five days old, ran on the machine of the macOS CI job. The system gives a port of the same range to a test that binds port 0. A listener of a test that got one of those ports accepted a PostgreSQL connection of such a server before the connection of its own client: `stream::socket::tests::an_answer_that_is_not_the_upgrade_of_this_request_opens_no_connection` failed once in CI with an unexpected end of the connection, and a listener that bound new ports every half second received 371 such connections from ten of the servers in two and a half minutes.

The tests of `tools/windows/start-run.ps1` replace the commands that change scheduled tasks with functions. PowerShell loads the module of a command at the first use of one of its commands, and that load replaces the functions of the same names. The start test that calls the real `New-ScheduledTaskAction` therefore registered and started a real task `\junto-ci\sample-run` in each run (measured: such a task, registered from the temporary folder of a test of a CI run). The removal test of a later run of these tests on the machine removed that task and failed, unless the start of a CI run had removed it first. A task that a process without elevation registered can be removed by the tests of a CI run, which run without elevation; one that an elevated process registered cannot (measured).

## Decision

- The server has the setting `JUNTO_STOP_AT_INPUT_END` (ADR 0008). The harness starts each server with it set to `true` and with a pipe on its standard input, whose write end only the process of the tests holds. The system closes that end when the process of the tests ends in any way, also by SIGKILL or `TerminateProcess`, and the server then stops as on SIGTERM. A test of the client library starts its own test program again as a process that starts a server, kills that process, and requires that the server has no session on its database within 30 seconds.
- A test that replaces a command of a PowerShell module loads that module first (`Import-Module`), so that no later load replaces the function. In the tests of `start-run.ps1`, the replaced registration and start of a task record their calls, and the start test requires both records.
- The tests of `start-run.ps1` use a run name that is new in each process of the tests. A task of another run, or a task that an earlier version of the tests left, cannot change their result, and they change no task of another run. After each start test, a task of that name is removed, and the test fails when one existed.

## Consequences

- A server of a test process that was killed stops within its graceful shutdown, and the port of its database is not polled afterwards.
- With the setting `true`, a server whose standard input is empty or closed from its start stops at once. The deployments do not set it.
- The rule does not stop processes that were started before it: the eleven servers were ended by hand.
- The test participants of calls and the window processes of the target `modes` are still stopped only in `Drop` of their test.
