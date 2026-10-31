# Install Junto on Linux

Junto runs on x86-64 Linux with PipeWire, under X11 or Wayland (ADR 0705). Obtain a package from the GitHub releases of the project, `https://github.com/luifon/junto/releases`, or from a source you have verified with whoever runs your server, and compare its SHA-256 with `SHA256SUMS` (`sha256sum -c --ignore-missing SHA256SUMS`). There are three packages with the same program (ADR 0754, ADR 0728):

- `junto_<version>_amd64.deb`: a Debian package, for Debian 12, Ubuntu 22.04 and later releases of both.
- `junto-<version>-linux-x86_64.tar.gz`: a directory `junto`.
- `Junto-<version>-x86_64.AppImage`: one file.

The packages carry the X11 keyboard libraries. They take from your system the C library (glibc 2.35 or newer), the X11 connection library, the graphics driver, the Wayland and font libraries, PipeWire and ALSA, and the desktop services (notifications, secret service, desktop portal). The AppImage needs `fusermount` of FUSE 2 or 3.

## The Debian package

Install it with the package manager, which also installs the libraries that it names:

```sh
sudo apt install ./junto_<version>_amd64.deb
```

Junto is then in the application menu, and `junto` starts it from a terminal. The program is in `/usr/lib/junto`. The package manager updates it: Junto shows the newer version below "Add server" and in Settings, Version, with the name of its `.deb` and the address of the download page; download that file and install it with the same command, then restart Junto. Junto does not update a `.deb` installation itself. Before removing it, quit Junto and run `junto --remove-startup-entry` as your usual desktop user. Then run `sudo apt remove junto`; your data stays in `~/.local/share/junto`.

## The tarball

Extract the directory into a writable directory of your account and start the program:

```sh
mkdir -p ~/.local/opt
tar -xzf junto-<version>-linux-x86_64.tar.gz -C ~/.local/opt
~/.local/opt/junto/junto
```

To add Junto to the application menu, run `~/.local/opt/junto/install-menu-entry`. It writes the menu entry and the icons into `~/.local/share` (or `$XDG_DATA_HOME`) with the path of this copy; run it again after you move the directory.

## The AppImage

Put the file in a writable directory of your account, make it executable and start it:

```sh
chmod +x Junto-<version>-x86_64.AppImage
./Junto-<version>-x86_64.AppImage
```

Closing the main window keeps Junto running with notifications and calls. Open it again from the tray. Use Quit to end it. Settings, Interface can turn off this behavior.

Settings, Interface also offers "Start Junto at login" for the normal profile, off until enabled. It uses your personal XDG autostart entry, normally `~/.config/autostart/chat.junto.Junto.desktop`. Updates and recovery keep it because the executable or AppImage path stays the same. If you move the installation, enable the setting again from its new location. Before removing any package, quit Junto and run its executable with `--remove-startup-entry` as your desktop user (for example `~/.local/opt/junto/junto --remove-startup-entry` or `./Junto.AppImage --remove-startup-entry`). The command opens no window and removes only your personal entry when it names that installation. If a matching system autostart entry is inherited, it writes a personal disabled override so reinstalling does not reactivate it. It preserves another installation's entry, other users' preferences and the system entry. Package-manager removal itself never edits users' home directories.

## Data and updates

The server book and the device keys are in `~/.local/share/junto` (or `$XDG_DATA_HOME/junto`, ADR 0710). A package never writes there.

A configured build checks for a new version at start and periodically. The server book shows the version of Junto below "Add server"; when a newer version exists, the tarball and the AppImage choose Update there, on the notice of the channel column, or Install update in Settings, Version. Junto verifies the signed release information, its date and the download, then replaces the directory or the AppImage file in one step. The AppImage keeps its file name. The update keeps the replaced version as an archive in a hidden directory beside the directory or the AppImage, named after it: `.junto.junto-update` for the directory `junto`, `.<file name>.junto-update` for an AppImage. The directory that holds them must stay writable. The first update also downloads the installed version from the release source for that purpose; when the source does not have it, the update is still installed and Version states that no previous version is kept. Then choose Restart: Junto quits and starts the new version; an AppImage starts again from its file.

An invite link can open Junto with the server and code filled. Review the server and choose Join; opening the link does not spend the code. A browser invite page also shows the code and the download page selected by the server administrator.

The Debian package declares Junto as a handler of `junto://` links. For the tarball, run its menu-entry script; for an AppImage, add its desktop entry with your desktop's integration tool. If the desktop already prefers another handler, select Junto in its default-application settings.

## Log and error reports

Junto writes its log to `logs/junto.log` in its data directory (at most three files of 4 MiB: `junto.log`, `junto.log.1`, `junto.log.2`). When an error stops an action of Junto, it writes a report to `panics/` in the same directory, continues, and names the report in a notice at the bottom of the window. Nothing of either is sent anywhere; send a report to whoever runs your server only if you choose to. `JUNTO_LOG_LEVEL` sets the level of the log; by default the log holds the information lines of Junto and the warnings of the libraries it uses. Starting Junto again with the same data directory restores the running window and exits the second process. When Junto cannot open its data directory at the start, it writes the reason to `junto-start-failure.txt` in the temporary directory (`$TMPDIR` or `/tmp`) (ADR 0757).

## Recovery

If the new version cannot start, quit every instance of Junto and run the recovery program kept beside the application, with the absolute path of the directory or of the AppImage file:

```sh
~/.local/opt/.junto.junto-update/record/previous/recover rollback ~/.local/opt/junto
```

It does not run the current program. It verifies the kept release signatures and archive bytes and puts the previous version back. It needs no network connection. It restores the program only; an older client may refuse a newer data-file format. If verification refuses recovery, keep the `.junto-update` directory and obtain an authentic package from an independently verified source.

An interruption can leave `stage-<random>` directories and `download-<random>` files in the `.junto-update` directory. Keep a staging directory until the installed program and the record have been checked: it can contain the only recovery copy.
