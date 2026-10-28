# 0754. Linux packages: tarball and AppImage

## Status

Accepted

Built and checked in the Linux build image on arm64 (ADR 0750); the Linux job of CI runs the same check on x86-64 (ADR 0905). Not run: a desktop with a real display server and graphics driver, an AppImage mounted through FUSE, a menu entry in a desktop environment.

## Context

ADR 0705 names two Linux packages, a tarball and an AppImage, and the rule of which libraries a package carries. ADR 0726 leaves the installation step of an update on Linux to the Linux packages. The client finds its data directory through the XDG variables (ADR 0710). A Linux binary linked in the build image needs these shared libraries: `libxcb`, `libxkbcommon`, `libxkbcommon-x11`, `libpipewire-0.3`, `libasound`, and the C library with `libm` and `libgcc_s`; `libxcb` and `libxkbcommon-x11` need `libxcb-xkb`, `libXau`, `libXdmcp`, `libbsd` and `libmd`. The window system, the graphics drivers and the session services are loaded at run time and are not in this list.

## Decision

The directory package:

- One directory `junto` with the client `junto`, the recovery program `junto-recover` (ADR 0726), the description `junto-package.json` (format 1, version, system `linux`, package `directory`, architecture), `lib/` with the bundled libraries, the menu entry `share/applications/chat.junto.Junto.desktop`, the icon theme `share/icons/hicolor` with the SVG source of the icon in `scalable/apps/chat.junto.Junto.svg` and its renderings of 16, 22, 24, 32, 48, 64, 128, 256 and 512 pixels in `<size>x<size>/apps/chat.junto.Junto.png` (ADR 0756), the script `install-menu-entry`, and `licences/` with the licence of Junto and the licence file of the package (ADR 0801), which also holds the copyright files of the Debian packages of the bundled libraries and, in the AppImage, the licences of its runtime. The AppImage also has `licences/appimage-runtime` with the source of its runtime (ADR 0801).
- The tarball `junto-<version>-linux-<architecture>.tar.gz` holds that directory. Every entry is owned by root, has the time of `SOURCE_DATE_EPOCH` (0 when it is not set), and no entry is a link.
- The client has the search path `$ORIGIN/lib` as `DT_RPATH`, which also applies to the libraries that the bundled libraries load: `libxcb-xkb` finds the host's `libxcb` through the system paths. A build outside a package has no such directory, and the loader takes the libraries of the system.
- `install-menu-entry` writes the menu entry and every size of the icon into `$XDG_DATA_HOME` (or `~/.local/share`) with the absolute path of this copy. It is run by the person and again after the directory is moved. The identifier `chat.junto.Junto` is the application identifier of the window (Wayland `app_id`, X11 class), so the desktop matches the window to the entry.

The AppImage:

- `Junto-<version>-<architecture>.AppImage` is an AppImage of type 2: the runtime of the AppImage project followed by a SquashFS image (zstd) of the same files as the directory, with `AppRun` as a link to `junto`, the menu entry and the SVG icon at the top, and the rendering of 256 pixels as `.DirIcon`.
- The runtime is the type 2 runtime of `AppImage/type2-runtime` at the commit of its release `20251108`, built by the Linux build image from the source archives of the runtime and of the libraries that it links statically, each checked by its SHA-256, with `tools/linux/appimage-runtime/build.sh` (ADR 0801); the SquashFS tools, the musl C library and the compiler-rt of LLVM are Debian packages of the snapshot of ADR 0750. The AppImage also carries every file of the link of the runtime and the script that links them. `tools/check-pins.sh` fails for a download of the image without a SHA-256 check of each file. The runtime is linked statically and needs `fusermount` (FUSE 2 or 3) from the host, not `libfuse.so.2`.

Libraries (the rule of ADR 0705, as names):

- Bundled: `libxkbcommon.so.0`, `libxkbcommon-x11.so.0` and `libxcb-xkb.so.1`. A host without an X11 keyboard library often lacks them.
- From the host: the C library and its loader, `libm`, `libgcc_s`; `libxcb.so.1` with `libXau`, `libXdmcp`, `libbsd` and `libmd`; `libpipewire-0.3` and `libasound`. The Mesa and Vulkan drivers of the host load `libxcb-dri3`, `libxcb-present` and other extensions that are linked against the host's `libxcb`; an older bundled `libxcb` loaded first through the search path could lack symbols that they need. The list of excluded libraries of the AppImage project names `libxcb.so.1` for this reason. Both sound libraries load their plugins and configuration from the host, so a bundled copy would not match them.
- `packaging/linux.py` takes the program `junto-icon` of the same build with `--icon-tool` and renders the icon theme with it. It reads the libraries of the client with `ldd` and stops when one is in neither list, so that a new dependency gets a decision. Its output is a new directory; it makes the missing parent directories of that directory, because `packaging/release_linux.sh target/linux-release` (`packaging/README.md`) runs in a new clone, which has no `target/`.

Updates:

- The update archive of the directory package holds the directory `junto`; the installer exchanges the installed directory and the candidate in one step with `renameat2(RENAME_EXCHANGE)`. A file system without that operation gets two renames, with a moment in which the path of the application does not exist.
- The update archive of the AppImage holds `Junto.AppImage`, `junto-recover` and `junto-package.json`. The installer exchanges the AppImage file in one step; the running AppImage keeps its open file and is not changed. The file keeps the name that the person gave it.
- The version of an AppImage is known only to the program that it runs (the runtime sets `APPIMAGE` and `APPDIR`). An update from the window uses that version. The recovery program does not know it, and the record decides as for an application that does not state its version (ADR 0726).
- The record and the staging directories are in `.<name>.junto-update` beside the directory or the AppImage file, on its file system (ADR 0726). The data directory is `$XDG_DATA_HOME/junto` or `~/.local/share/junto` (ADR 0710).

Checks (`packaging/check_linux.py`, in the build image):

- The tarball: its entries, the owner, no links, the bundled libraries exactly, no library of the host, the description, the licences, a licence file that lists every crate of the programs and holds the WebRTC licence and the copyright files of the bundled libraries; the search path; with an empty `LD_LIBRARY_PATH` every bundled library loads from the package and every other one from the host.
- The first frame of the tarball's client under X11 and Wayland, and of the AppImage under X11, with `--check-startup` on the virtual displays of ADR 0750. The container has no FUSE: the AppImage runs with `APPIMAGE_EXTRACT_AND_RUN=1`.
- The menu entry script, and the files of the AppImage after `--appimage-extract`, with its licence file, which also holds the licences of its runtime, and the source of the runtime, which builds the runtime of the AppImage again, byte for byte. The icon theme of the tarball, of the AppImage and of the installed menu entry, and `.DirIcon`, equal the renderings of the icon program.
- An update of a running fixture directory package from a local source and its rollback with the recovery program; a read-only location, a changed archive and a second rollback are refused.
- The Rust tests `tests/linux_packages.rs` replace an AppImage that is open and a directory with the Linux system tools, and roll both back.

- The real-client lifecycle (`packaging/check_lifecycle.py`, described in ADR 0725) for the tarball and for the AppImage: two builds of the checkout with temporary test keys, the workspace version and its next patch version, in their own target directory. The first package is installed in a folder whose name holds a space and a character outside ASCII. The window of the installed client finds and installs the update with `--update-source` and `--check-update` on the X11 display, then presses Restart and requires a different process to draw its first frame with the new version (ADR 0724, ADR 0780); the AppImage runs through its runtime, so the client finds its AppImage and its version as it does for a person. The recovery program of the previous release record rolls the update back. At installation, after the update and after the rollback, the installed directory equals the package directory of the expected version file by file, or the AppImage equals the expected AppImage, and the client states its version and draws its first frame under X11 and under Wayland, each start with a new empty data directory.
- The lifecycle uses FUSE when the container has a `/dev/fuse` that the account can read and write and a `fusermount` or `fusermount3` program. Otherwise it prints the missing prerequisite and uses extract-and-run, with the extraction under its temporary folder; the build container of CI has no `/dev/fuse`. A start and `--version` have a bound of 300 seconds, as the other starts of this check: a start includes the start of the virtual display and its window manager, and with extract-and-run each run of the AppImage includes its extraction. `--version` runs without a virtual display; the update and restart use X11. The processes of every command, with its virtual display, are ended when the command returns. A Linux restart driver keeps the display alive and adopts the detached restart helper and its descendants until they end; its deadline leaves time for cleanup before the outer command deadline. No package, menu entry, data directory or extraction is written outside the temporary folder.

## Consequences

- A person extracts the tarball into a directory of their account, or makes the AppImage executable, and keeps the directory writable for updates.
- A new shared library of the client stops the package build until it is added to one of the two lists and to this record.
- The AppImage runs only where `fusermount` exists, or with `--appimage-extract-and-run`. A desktop with Mesa older than 25 under X11 gets the start rule of ADR 0705; the bundled libraries do not change it.
- A Debian package is made beside the tarball and the AppImage, from the same directory (ADR 0728). An RPM package is not made (`docs/scope.md`).
