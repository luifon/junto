# 0915. A test leaves no process and no scheduled task behind

## Status

Accepted

## Context

The tests start the server binary of the checkout as a process of their own through the harness in `crates/client-core/tests/e2e/support.rs`: the end-to-end tests of the client library, the window tests and the native targets of the client (`frames`, `endurance`, `modes`, `rehearsal`), the call tests of the media engine (`crates/media/tests/calls`) and its native device stages (`crates/media/tests/native`). Each server uses a database of the PostgreSQL that `tools/with-test-postgres.sh` starts, which Docker publishes on a free port of the loopback address. The harness stopped a server only in `Drop`. A process of the tests that a signal ends (SIGKILL, SIGTERM, the end of its terminal) runs no `Drop`, so its servers kept running. The wrapper then removed the PostgreSQL container, and each such server kept connecting to the freed port. Eleven such servers, from test runs one to five days old, ran on the machine of the macOS CI job. The system gives a port of the same range to a test that binds port 0. A listener of a test that got one of those ports accepted a PostgreSQL connection of such a server before the connection of its own client: `stream::socket::tests::an_answer_that_is_not_the_upgrade_of_this_request_opens_no_connection` failed once in CI with an unexpected end of the connection, and a listener that bound new ports every half second received 371 such connections from ten of the servers in two and a half minutes.

A rule that stops the server at the end of a pipe that the process of the tests holds does not hold on macOS: the standard library creates a pipe and makes its ends private to the process in a second call, so a process that another thread of the tests starts between the two calls inherits the end, and the server runs until that process ends. On Windows the standard library creates the inherited end of a pipe while no other child is started.

The tests of `tools/windows/start-run.ps1` replace the commands that change scheduled tasks with functions. PowerShell loads the module of a command at the first use of one of its commands, and that load replaces the functions of the same names. The start test that calls the real `New-ScheduledTaskAction` therefore registered and started a real task `\junto-ci\sample-run` in each run (measured: such a task, registered from the temporary folder of a test of a CI run). The removal test of a later run of these tests on the machine removed that task and failed, unless the start of a CI run had removed it first. A task that a process without elevation registered can be removed by the tests of a CI run, which run without elevation; one that an elevated process registered cannot (measured).

## Decision

- The server has the setting `JUNTO_STOP_WITH_PROCESS`: the number of the process that started it (ADR 0008). The server stops as on SIGTERM when that process ends, also when it is killed.
  - On Unix systems the server reads the number of its parent every 100 ms and stops when it differs. When a process ends, the system gives each of its children another parent, the first process or a process that collects orphans; that process already ran, so it cannot have the number of the ended one. No descriptor that another process holds delays this, and the calls are safe calls of `rustix`.
  - On Windows the server opens the process with that number, and waits for its end when it was created before the server; otherwise the process that started the server has ended and Windows gave its number to another process, and the server stops at once. The calls go through `windows-sys` in a module of its own, the second module of the server with unsafe code (ADR 0605 has the first).
  - The harness starts each server with the number of the process of the tests, and so does the server test that starts the binary to read its standard output (`first_start::the_binary_writes_the_notice_of_the_owner_code_in_one_write`). A test of the client library starts its own test program again as a process that starts a server and `sleep` processes that each hold a copy of one pipe of that process (the inheritance that the race on macOS gives), kills that process, and requires that the server has no session on its database within 30 seconds.
- A test server accepts only the connection that it expects. The tests of the event stream connection in `junto-client-core` put a value that is new in each call into the path of the request, read each accepted connection in a task of its own, and close every connection whose request line does not name that path.
- A test that replaces a command of a PowerShell module loads that module first (`Import-Module`), so that no later load replaces the function. In the tests of `start-run.ps1`, the replaced registration and start of a task record their calls, and the start test requires both records.
- The tests of `start-run.ps1` use a run name that is new in each process of the tests. A task of another run, or a task that an earlier version of the tests left, cannot change their result, and they change no task of another run. After each start test, a task of that name is removed, and the test fails when one existed.

## Consequences

- A server of a test process that was killed stops within 100 ms and its graceful shutdown, and the port of its database is not connected to afterwards.
- A server started with the setting by a program that is not its parent on a Unix system stops at once. The deployments do not set it.
- Servers that were started before this change do not stop by this rule.
- The test participants of calls and the window processes of the target `modes` are still stopped only in `Drop` of their test.
