# 0906. A bound on the stack that one function needs

## Status

Accepted

## Context

A thread of the test harness and a worker thread of the runtime of the server have a stack of 2 MiB. In a build without optimization on Linux (arm64) the server test `calls_review::every_request_that_changes_a_call_is_limited` ended the test process once with a stack overflow. Alone it passed with a stack of 2048 KiB and overflowed with 1792 KiB.

The stack was not used by a deep chain of calls or by a large future: the largest future that clippy finds in the server has 8,872 bytes. Two kinds of functions needed it. A build without optimization gives every value of an expression a place of its own on the stack and uses no place twice.

- `routes::documented_routes` made the 140 groups of routes of the server in one expression. The test that builds the routes with their schemas passed with 1792 KiB and overflowed with 1536 KiB. clippy counted 1,037,387 bytes for the values of the function.
- The schema functions that the derive macro of utoipa makes for the large enums of the protocol (`AuditEvent`, `Event`, `StructureChange`) are one expression over every variant. With the first function corrected, the same test still overflowed with 768 KiB, in the schema function of `AuditEvent`.

A build with optimization uses a place on the stack again. The release build of the same test passed with 128 KiB, the smallest stack that the system gives a thread. The server binary builds its routes once, at its start, on the main thread, whose stack is 8 MiB. The released server was therefore not near a limit. A server built without optimization had about 6 MiB left on its main thread. Near the limit were the tests, which start the server on a thread of the test harness, and any program that builds the routes on a thread with 2 MiB.

## Decision

- Each group of routes is made in a function of its own, which returns before the next one is called (`routes::add` takes the function). The stack that the routes need no longer grows with their number.
- The protocol crate is compiled with optimization level 1 also in the dev profile. Its generated schema functions cannot be divided without another form of the types that they describe, and with optimization they need little stack.
- `clippy::large_stack_frames` is denied in every crate of the workspace, with a bound of 262,144 bytes in `clippy.toml`: one eighth of the stack of a thread. clippy counts the values of a function before optimization. The count is lower than the stack that the build needs: the corrected function was counted at 1.04 MB and needed about 1.6 MiB. The lint step of CI applies the bound. The generated schema functions of `AuditEvent` and of `Event` are above the bound and are expected in their modules; the one of `StructureChange` is at the bound and is allowed in its module.
- A unit test of the server builds the documented routes with their schemas on a thread with a stack of 512 KiB, one quarter of the stack of a thread. It measures the code as it is built, with the generated functions and the code of the dependencies, which the lint does not count. An overflow ends the test process and names the thread of the test.

Not chosen: a larger stack for the threads of the tests, or `RUST_MIN_STACK` in a test environment. The need grew with each route and with each variant of the audit log, so a larger stack moves the limit and does not show the growth. `clippy::large_futures` was not chosen because no future was large.

## Consequences

- After the change the test that builds the routes passes with 192 KiB in a build without optimization on Linux and on macOS, and the call test above passes with 512 KiB.
- The protocol crate is optimized in every local build. Its build takes longer, and a debugger shows its code less exactly.
- A new function above the bound fails the lint step. A generated function is then expected or allowed in its module with its reason, and the unit test still bounds what the build needs.
- `large_stack_frames` belongs to the nursery group of clippy: its count is an estimate and can change with the Rust version, which `rust-toolchain.toml` fixes.
