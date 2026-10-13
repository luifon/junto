# 0705. Supported systems and packages

## Status

Accepted, pending step 0

Open: certificate-signed macOS product bundles and first-install ZIPs were built, with update and rollback tests in a temporary directory (ADRs 0725–0726). The packaged install on a clean system was not run on any system (`docs/step-0/README.md`, test 8). No test of step 0 has run on Windows.

## Context

The client depends on operating-system functions that differ between systems and versions: screen capture, sound capture, the key store and the window system (ADR 0502, ADR 0103, ADR 0701). Each supported system must be tested on real hardware or a real installation (ADR 0902), so the list of supported systems sets the amount of testing.

Step 0 ran a test application on macOS 26 and on a Linux desktop, and recorded which libraries the Linux binary needs (`docs/step-0/README.md`, test 2).

## Decision

- macOS: macOS 26 on Apple Silicon. This is the tested version. Older versions are not blocked and are not tested. Screen share with sound needs macOS 13 or later (ADR 0502).
- Windows: Windows 11 on x86-64.
- Linux: x86-64 with PipeWire, on X11 or on Wayland.
- Linux packages: a tarball and an AppImage.
- A Linux package bundles the libraries that the binary links and that do not depend on the host: libxcb, libxkbcommon and libxkbcommon-x11 with the libraries they need, and the interface fonts.
- A Linux package takes from the host: the C library, the graphics driver with its Vulkan or OpenGL loader, the Wayland client library, the font configuration and system fonts, the keyboard data, the certificate bundle, and the session services (notifications, secret service, desktop portal, accessibility). A graphics driver must match the kernel of the machine, so it cannot be bundled. The PipeWire library also comes from the host, because it loads its plugins from the host; this follows from how the library works and was not tested with a package.
- The Linux build runs on a base system with an old C library. A build on Debian 12 needs glibc 2.35 or newer on the user's system.

## Consequences

- A problem on a macOS version older than 26 can exist without being found by the project's tests.
- A system outside this list is not supported and is not tested.
- Linux is tested in two sessions, X11 and Wayland. Some functions differ on Wayland: screen capture goes through the system portal (ADR 0701), and the call window cannot be kept above other windows (ADR 0507).
- When the host has no usable GPU driver and has a software renderer, gpui selects the software renderer and the client runs. Video is reduced there (ADR 0701).
- The test binary ran on the test desktop without an installed package. The lists above come from that binary; no tarball and no AppImage was built.
- An RPM package is in the later list of `docs/scope.md`.
