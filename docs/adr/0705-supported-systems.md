# 0705. Supported systems and packages

## Status

Accepted, pending step 0

## Context

The client depends on operating-system functions that differ between systems and versions: screen capture, sound capture, the key store and the window system (ADR 0502, ADR 0103, ADR 0701). Each supported system must be tested on real hardware or a real installation (ADR 0902), so the list of supported systems sets the amount of testing.

## Decision

- macOS: macOS 26 on Apple Silicon. This is the tested version. Older versions are not blocked and are not tested.
- Windows: Windows 11 on x86-64.
- Linux: x86-64 with PipeWire, on X11 or on Wayland.
- Linux packages: a tarball and an AppImage.
- Step 0 tests a packaged install on a clean system of each kind (`docs/step-0/README.md`, test 8).

## Consequences

- A problem on a macOS version older than 26 can exist without being found by the project's tests.
- A system outside this list is not supported and is not tested.
- Linux is tested in two sessions, X11 and Wayland. Some functions differ on Wayland: screen capture goes through the system portal (ADR 0701), and the call window cannot be kept above other windows (ADR 0507).
- An RPM package is in the later list of `docs/scope.md`.
