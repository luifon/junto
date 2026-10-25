# 0756. The application icon and its renderings

## Status

Accepted

The macOS icon file, the Linux packages and the Windows resource file are built and checked on macOS and in the Linux build image (ADR 0750). The icon group of a Windows program built with the MSVC linker is checked by the Windows job of CI only. On macOS 26 the icon of a bundle with a built client, as `NSWorkspace` gives it to Finder, is the drawing with the edge highlights that the system adds to application icons. Not checked: the icon that Explorer, the taskbar, a Linux menu or an X11 window manager shows on a real desktop.

## Context

Each system reads the application icon from its own place and file format. macOS reads `Junto.icns` from the bundle (ADR 0725). Windows Explorer shows the first icon group of a program, and gpui gives the icon group number 1 of the program to the window class of its windows. Linux desktops read the icon of the menu entry from the `hicolor` icon theme (ADR 0754); a Wayland compositor finds the menu entry by the application identifier of the window; an X11 window manager can also read the icon of a window from its `_NET_WM_ICON` property, which gpui sets from the icon of the window options and only on X11.

A raster that is made by hand for each size and committed can differ from the source after a change of the design, and a reviewer cannot read it. The client already depends on resvg, the SVG renderer of gpui, and `Cargo.lock` pins its version.

## Decision

The source:

- `packaging/icon/junto.svg`, 1024 by 1024: three overlapping discs in terracotta, ochre and cypress, whose shared area is light, on the limestone background `#E3D7C1`, in a rounded square with the margin and the shadow of the macOS icon grid. Every package of every system uses this file.
- `packaging/icon/junto-square.svg` is the same drawing as a full square without transparency, for a place that needs a square, such as a web page or a store listing. No package uses it.

The renderings:

- The crate `junto-icon` (`crates/icon`) renders the source with resvg in the version that gpui uses, without the features that read fonts and raster pictures. It makes a PNG picture of one size, the macOS icon file, the Windows icon file and a compiled Windows resource file (`.res`). Its program `junto-icon` writes one of these into a new file.
- No raster is committed. The packaging scripts take the program of the same build: `packaging/bundle.py` as its fourth argument, `packaging/linux.py` with `--icon-tool`; the release scripts build it with the client. The build scripts of the client and of the client core have the crate as a build dependency.
- The renderer is the feature `render` of the crate, on by default; the program `junto-icon` needs it. The build dependencies turn it on only where their build script uses a rendering: the client on Windows and Linux, the client core on Windows. Without the feature every rendering fails with an error that names the feature, so a build script never writes an empty icon. Cargo selects a build dependency of a `[target]` section by the system that runs the build, not by the target of the build. Every build of a package runs on its own system (ADR 0750, ADR 0753), so the two are the same; a build for Windows or Linux on another system stops in the build script with that error.
- The standalone macOS signing and update checks build `junto-icon` before assembling a bundle, so they work after a clean build directory and rebuild after a change of the SVG source.
- The pictures of 16 pixels are reductions of the drawing of 1024 pixels. There is no simplified drawing for small sizes.

macOS (`Contents/Resources/Junto.icns`):

- Each size of the macOS icon grid from 16 to 512 points at 1x and at 2x, with the types that `iconutil` of macOS writes: `ic04` (16) and `ic05` (32) as `ARGB` with the run-length code of the icon file, and PNG pictures in `ic11` (32), `ic12` (64), `ic07` (128), `ic13` and `ic08` (256), `ic14` and `ic09` (512), `ic10` (1024). With PNG pictures in the types `icp4` and `icp5` instead, `iconutil` did not read them as PNG: the picture of 16 pixels that it wrote back was opaque, with other colours, where the rendering is transparent.
- The straight colour channels of the ARGB entries are computed from the renderer's premultiplied pixels with integer division and half values rounded up, matching `iconutil`. Floating-point demultiplication can round a half value down; in the rendering of 16 pixels this changes six blue values from 128 to 127.

Windows (the programs `junto.exe` and `junto-recover.exe`):

- The build script writes a compiled resource file with one icon resource for each of 16, 24, 32, 48, 64 and 256 pixels (numbers 1 to 6) and the icon group number 1 that lists them, in the neutral language, and gives it to the MSVC linker for the program `junto` only. The build script of `junto-client-core` gives the same resource file to the linker for its program `junto-recover`, the recovery program of the package (ADR 0755), so that Explorer shows the icon of Junto for both programs of the installation folder. The other programs of that crate, `junto-cli` and `junto-release`, are not in a package and have no icon. The pictures up to 64 pixels are bitmaps of 32 bits per pixel with the mask of their transparent pixels; the picture of 256 pixels is PNG. `junto-icon ico` writes the same pictures as an icon file.
- The MSVC linker reads a compiled resource file without the resource compiler of the Windows SDK. A build for another Windows target stops in the build script, because ADR 0753 builds for MSVC only.

Linux (the tarball and the AppImage):

- `share/icons/hicolor/scalable/apps/chat.junto.Junto.svg` is the source, and `share/icons/hicolor/<size>x<size>/apps/chat.junto.Junto.png` is its rendering for 16, 22, 24, 32, 48, 64, 128, 256 and 512. The AppImage has the source at its top as `chat.junto.Junto.svg` and the rendering of 256 as `.DirIcon`. `install-menu-entry` copies every size into the icon theme of the person.

The icon of a window:

- On Linux the build script writes the rendering of 64 pixels, the client carries it in the binary, decodes it once and gives it to the main window and to the detached call window. On X11 gpui sets it as `_NET_WM_ICON`, so a window manager shows it also when no menu entry is installed. A Wayland compositor takes the icon from the menu entry of the application identifier: the detached call window has the identifier of the main window.
- On macOS the system takes the icon from the bundle, and on Windows gpui takes it from the icon group of the program; the window options there have no icon.

The picture of a notification on Windows:

- The build script of the client on Windows also writes the rendering of 256 pixels (`junto_icon::NOTIFICATION`), and the client carries it in the binary. At each start it writes the picture as `notification-icon.png` into its data directory when the file differs, and the key of the identity of its notifications names that file (ADR 0742): a notification of a program without a package takes its picture from a file.

Checks:

- The tests of `junto-icon`: a rendering of 1024 and of 32 pixels has the colour of the source at a point inside each area of the drawing (each disc alone, each pair, all three, the background), transparent corners and the shadow below the square; each entry of both icon files and of the resource file holds the rendering of its size.
- The packaging tests: the icon file of a bundle holds the rendering of each entry; on macOS `iconutil` reads all ten sizes; its picture of 64 pixels differs from the rendering by at most one step of a channel, and in its pictures of 16 and 32 pixels the alpha of every pixel and the colour of every opaque pixel do. From an `ARGB` entry `iconutil` writes other colours for pixels that are partly transparent, also when it reads the icon file that it wrote itself from the same pictures; the icon theme of the Linux packages and the script of the menu entry; the Windows icon file; and, where the LLVM linker for Windows programs exists (the Linux build image), a library of resources only that this linker makes from the resource file holds the icon group 1 with each rendering.
- `packaging/check_linux.py` compares the icon theme of the tarball, of the AppImage and of the installed menu entry, and `.DirIcon`, with the renderings; `packaging/check_windows.py` reads the icon group 1 of `junto.exe` and of `junto-recover.exe` from the ZIP and compares each picture with the rendering, and after a start of the installed client compares the picture of the notifications in the data directory with the rendering of 256 pixels. A unit test of the client on Linux decodes the window icon.
- On macOS a packaging test builds an iconset from the same PNG renderings and runs `iconutil -c icns`. It decodes the run-length channels of `ic04` and `ic05` from both files and compares all four channels at every partly transparent pixel. This compares the stored values directly, before the colour changes of reading an icon file back as PNG. The test states its requirement for macOS and `iconutil` when skipped elsewhere.

## Consequences

- A change of the design is a change of `packaging/icon/junto.svg`; every package gets the new renderings at its next build, and the tests of the crate state the colours and the areas of the drawing.
- A build of the client compiles resvg for its build script on Windows and Linux, and a build of the client core on Windows only. A build on macOS compiles it only for gpui and for the program `junto-icon`, not for a build script.
- The icon is the project's own work, under the licence of the repository (Apache-2.0).
