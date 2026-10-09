# 0902. Testing strategy

## Status

Accepted

## Context

The client runs on three operating systems, and calls, recording and transcripts involve several components (ADR 0001). A call cannot be tested automatically with real people, real microphones and real screens. Real capture of screen, sound and microphone uses operating-system functions that a test participant without a user interface does not use.

## Decision

Automated tests:

- CI builds and tests on macOS, Windows and Linux for every commit.
- A test participant without a user interface publishes a known tone and a known test picture into a call. The test records the call and checks the file.
- Tests cover reconnects, a recorder that stops during a recording, and a restore from a backup (ADR 0005).
- Tests run the client against the current and the previous API version (ADR 0201).
- CI regenerates the API documentation and fails on a difference (ADR 0202), and checks the licence of every dependency (ADR 0801).

Tests on real systems:

- Real capture of screen, sound and microphone is tested on each operating system:
  - a real macOS machine;
  - a real Linux desktop, in an X11 session first and then in a Wayland session;
  - a Windows 11 virtual machine. Its host needs hardware virtualization enabled in its firmware.
- The packaged application is installed and updated on a clean system of each kind.
- Real microphones and GPUs are tested on physical machines before a release.

## Consequences

- The recording path is tested from a published signal to the content of the output file, without a person.
- Capture on real systems is tested by a person and is not part of CI, so an error in capture can pass CI.
- A release needs a macOS machine, a Linux desktop with both session types, a Windows 11 virtual machine and a clean system of each kind.
- Capture on Windows is tested in a virtual machine. Real microphones and GPUs on Windows are covered only by the physical-machine tests before a release.
