# 0728. Installers of the first install

## Status

Accepted

The unit tests of the setup program (`windows_setup` of `junto-client-core`), of the disk image (`test_dmg.py`) and of the Debian package (`test_linux.py`) run on Linux, and the Debian package is checked in the Linux build image (`check_linux.py`). The disk image is checked by `check_release.py` on macOS and the setup program by `check_windows.py` on Windows; both need those systems.

## Context

ADR 0755 and ADR 0754 give each system a package that a person extracts: a ZIP file, a tarball, an AppImage; on macOS a ZIP file with `Junto.app` (ADR 0725). Discord, Slack and Teams offer an installer per system: a disk image on macOS, a setup program on Windows that installs for the account and starts the application, and a `.deb` on Linux. A person expects those, with an entry in the Start menu and in the list of installed programs on Windows, and with a menu entry on Linux.

The update of ADR 0726 replaces the installation in place, so an installer must put the files into the layout that an update expects. The project buys no code-signing certificate (ADR 0704).

For the Windows setup program the options were:

- Inno Setup or NSIS: a setup program that copies files, with a wizard, its own uninstall program and log in the folder. The tool is one more download to pin and install on the Windows machine of CI. Its uninstall program and log are files of the folder that the update archive does not hold, so the first update by entries (ADR 0755) would remove them.
- WiX (MSI): ADR 0755 states why an MSI does not fit the update of a running program; it also needs the WiX tool set.
- A program of the workspace: built with the toolchain and the lock file of the checkout, as the client is, on every Windows build; it reuses the archive format and the replacement by entries of the client core.

## Decision

macOS:

- `packaging/dmg.py` writes `Junto-<version>-macos-aarch64.dmg`: a compressed read-only image (UDZO, HFS+) with the volume name `Junto` that holds the signed `Junto.app`, copied with `ditto`, and a link `Applications` to `/Applications`. The person opens it and drags Junto onto the link.
- The release command (`packaging/release_locked.sh`) writes it beside the ZIP file, and `packaging/sign.py` signs the image with the project certificate (ADR 0725) and requires the certificate in its designated requirement. The update path stays the signed archive (ADR 0726); the image is for the first install.
- `packaging/check_dmg.py`, which `check_release.py` runs for each release that it builds, verifies the image with `hdiutil verify`, its signature and certificate, mounts it read-only without a Finder window, requires exactly `Junto.app` and the link to `/Applications`, verifies the bundle and requires the designated requirement of the bundle of the release, and detaches it also after a failure.

Windows:

- The setup program is the binary `junto-installer` of `junto-client-core`, a program of the Windows subsystem that carries the icon of the application (ADR 0756). `junto-release setup INSTALLER PACKAGE-DIRECTORY NEW-SETUP` writes `Junto-<version>-windows-x86_64-setup.exe`: the program, then the archive of the folder `Junto` in the format of ADR 0726, then its length, its SHA-256 and a mark (`releases::setup`). `packaging/windows.py --installer --release-tool` writes it beside the ZIP file. Its calls of the Windows API (the shortcut through COM, the questions through `MessageBoxW`, the list of processes) are in the module `releases::setup_windows`, which allows unsafe code; what the setup program decides is in `releases::setup`, without unsafe code, and its tests run on every system.
- The folder of the package also holds the same program as `uninstall.exe`, without an archive. The update archive holds the folder, so each update brings the uninstall program of its version.
- Run, the setup program checks the length and SHA-256 of its archive, asks the person to quit a Junto that runs from the folder, and installs into `%LOCALAPPDATA%\Programs\Junto` without administrator rights: the archive is extracted into a staging directory of `.Junto.junto-update` beside the folder, under the lock of the updates (ADR 0726), and the folder is put in place, or, when it exists, its entries are replaced one by one as an update replaces them (ADR 0755). A newer installed version is kept unless `--allow-older` is given. The setup writes a Start menu entry `Junto.lnk` that names the application identifier (ADR 0755) and the entry of the list of installed programs, `HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Uninstall\Junto`: name, version, publisher, icon, location, the uninstall command `"<folder>\uninstall.exe"` and the quiet one with `--quiet`, the size, and no modify or repair. Then it starts Junto (not with `--no-start`). It asks nothing else, as the setup programs of Discord and Slack do. `--quiet` shows no dialog and writes errors to standard error.
- The uninstall program asks whether to remove Junto, then whether to remove the data directory as well (the default keeps it), asks to quit a Junto that runs, and starts a copy of itself in the temporary folder, because Windows does not let a running program delete its own file. The copy removes the registration of the notifications (ADR 0742), the folder and the update directory, the Start menu entry and the entry of the installed programs when that entry names this folder (the uninstall program of a ZIP copy in another folder leaves those of a setup installation), and with the answer the data directory; it tries for 30 seconds while the uninstall program ends, then removes itself through `cmd.exe` after it ended. `--quiet` asks nothing and keeps the data unless `--remove-data` is given.
- At each start the client sets the version of the entry of the installed programs to its own version when the entry names its folder, because an update from inside the application replaces the folder (ADR 0726) without the setup program.
- `check_windows.py` builds the setup program of the client build, checks its subsystem and icon, runs it with `--quiet --no-start` with `LOCALAPPDATA`, `APPDATA` and `TEMP` in its temporary folder, and requires the folder to equal the package folder, the Start menu entry, the entry of the installed programs with its version, location and command, and the start of the installed program. A second run replaces the folder. The quiet uninstall program then removes every part within 60 seconds and leaves no copy in the temporary folder, once keeping and once removing the data directory. The check restores the entry of the installed programs as it was before (ADR 0915).

Linux:

- `packaging/linux.py` writes `junto_<version>_amd64.deb` beside the tarball with `dpkg-deb --root-owner-group`: the program directory of the tarball, without `junto-recover` and `install-menu-entry`, in `/usr/lib/junto`, with a description that names the package `deb`; `/usr/bin/junto`, a link to the program; the menu entry and the icon theme in `/usr/share`; the licence of Junto as `/usr/share/doc/junto/copyright`, with the licence file of the package in `/usr/lib/junto/licences`. `Depends` names the package of each library that the program takes from the host (ADR 0754), by its name in Debian 12 and the name that replaced it in later releases (`libasound2 | libasound2t64`); `Recommends` names the Vulkan driver, the desktop portal of a screen share and PipeWire, which are loaded at run time. The bundled libraries stay in the package.
- A `.deb` installation is updated by the package manager: the client finds the package name `deb` in the description of its folder and installs nothing itself; its version line names the `.deb` of the newer version, and the section "Version" of the settings gives the command `sudo apt install ./junto_<version>_amd64.deb` and the address of the download page (ADR 0780). The update check reads the release information of the directory package, which has the same version. No apt repository is published in this version: it would need a signing key of its own and a host for its index.
- `check_linux.py` requires the fields of the package, that the first name of each dependency is a package of the Debian release of the build image, the program of the tarball in `/usr/lib/junto`, the bundled libraries, the link, the menu entry, the icon theme and the copyright file, and the first frame of the extracted program through `/usr/bin/junto` under X11 and Wayland.

The release site (ADR 0727) and the GitHub release offer the three installers beside the packages of ADR 0754 and ADR 0755.

## Consequences

- A person on each system installs Junto in the way that Discord and Slack use. On Windows Junto is in the Start menu and in Settings, Apps; it is removed there.
- The setup program is unsigned, as every program of the project: SmartScreen shows its warning at its first start (ADR 0704, ADR 0755). The SHA-256 of its archive detects a damaged download, not a changed one; the checksums of the release site and the SHA256SUMS of the release cover the whole file.
- The archive of the setup program is not signed with the release keys and the setup writes no record of the installed release: the first update downloads the release of the installed version for its rollback (ADR 0726).
- A newer version of the `.deb` is installed by the person; the client cannot update a file that the package manager owns.
- Not tested: the setup program started from Explorer with its dialogs and the start of Junto at its end, the uninstall from Settings, Apps, the drag from the disk image in the Finder, and an installation of the `.deb` with `apt` on a desktop. The coordinator and a person check them on the real systems.
