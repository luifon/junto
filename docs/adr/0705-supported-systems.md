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
- The Linux container checks also run on ARM64 (ADR 0750). They do not establish package support or replace installation tests. The container baseline uses PipeWire 0.3.65 or newer and Mesa 25.0.7.
- Under X11 the client does not draw with the software Vulkan driver of Mesa older than 25. That driver in Mesa 22.3.6, which Debian 12 ships, presents a black window under X11, and gpui tries a Vulkan adapter before an OpenGL one and has no setting to exclude an adapter. Before its first window the client reads the Vulkan adapters. When none is a graphics processor and one is that driver, the client starts itself again with an empty list of Vulkan drivers, and gpui draws with OpenGL. Version 25 is the lowest version that was seen to draw (25.0.7); the versions between 22.3.6 and 25.0.7 were not tested and are treated as 22. A machine with a graphics processor, and a Wayland session, are not changed.
- Linux packages: a tarball, an AppImage (ADR 0754) and a Debian package (ADR 0728). Windows packages: a ZIP of one folder (ADR 0755) and a setup program that installs that folder for the account (ADR 0728). macOS: a ZIP and a disk image with `Junto.app` (ADR 0725, ADR 0728).
- A Linux package bundles libxkbcommon and libxkbcommon-x11 with libxcb-xkb (ADR 0754). The interface fonts are in the binary. Step 0 also named libxcb: it is taken from the host, because the graphics drivers of the host load other extensions of the X11 connection library, which are linked against the host's version of it, and an older bundled copy loaded first can lack symbols that they need.
- A Linux package takes from the host: the C library, the X11 connection library (libxcb with libXau, libXdmcp, libbsd and libmd), the graphics driver with its Vulkan or OpenGL loader, the Wayland client library, the font configuration and system fonts, the keyboard data, the certificate bundle, and the session services (notifications, secret service, desktop portal, accessibility). A graphics driver must match the kernel of the machine, so it cannot be bundled. The PipeWire library and the ALSA library also come from the host, because each loads its plugins and configuration from the host.
- The Linux build runs on a base system with an old C library. A build on Debian 12 needs glibc 2.35 or newer on the user's system.

## Consequences

- A problem on a macOS version older than 26 can exist without being found by the project's tests.
- A system outside this list is not supported and is not tested.
- Linux is tested in two sessions, X11 and Wayland. Some functions differ on Wayland: screen capture goes through the system portal (ADR 0701), and the call window cannot be kept above other windows (ADR 0507).
- When the host has no usable GPU driver and has a software renderer, gpui selects the software renderer and the client runs: with Vulkan, or with OpenGL where the rule above for X11 applies. Video is reduced there (ADR 0701).
- The check of the Vulkan adapters adds the start of the Vulkan loader to each start under X11, and the second start of the process on a machine with the old software driver. The process that was started again has the empty list of Vulkan drivers in its environment, and a program that it starts, for example to open a file, inherits it: such a program finds no Vulkan driver. Only a machine whose one Vulkan driver is the old software driver is in this case. It was run with the Vulkan driver of Mesa 22.3.6 beside the OpenGL driver of Mesa 25.0.7; a system with Mesa 22 alone was not run.
- A desktop whose portal has no screen-cast backend has no screen share (ADR 0751). This is the case for some X11 desktops.
- The lists above come from the test binary of step 0 and from the libraries that the client of the build image links (ADR 0754). The tarball and the AppImage ran in the build image on virtual displays; they did not run on a desktop with a real display server.
- An RPM package is in the later list of `docs/scope.md`.
