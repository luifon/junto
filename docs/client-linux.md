# Install Junto on Linux

Junto runs on x86-64 Linux with PipeWire, under X11 or Wayland (ADR 0705). Obtain a package from a source you have verified with whoever runs your server. There are two packages with the same program (ADR 0754):

- `junto-<version>-linux-x86_64.tar.gz`: a directory `junto`.
- `Junto-<version>-x86_64.AppImage`: one file.

The packages carry the X11 keyboard libraries. They take from your system the C library (glibc 2.35 or newer), the X11 connection library, the graphics driver, the Wayland and font libraries, PipeWire and ALSA, and the desktop services (notifications, secret service, desktop portal). The AppImage needs `fusermount` of FUSE 2 or 3.

## The tarball

Extract the directory into a writable directory of your account and start the program:

```sh
mkdir -p ~/.local/opt
tar -xzf junto-<version>-linux-x86_64.tar.gz -C ~/.local/opt
~/.local/opt/junto/junto
```

To add Junto to the application menu, run `~/.local/opt/junto/install-menu-entry`. It writes the menu entry and the icon into `~/.local/share` (or `$XDG_DATA_HOME`) with the path of this copy; run it again after you move the directory.

## The AppImage

Put the file in a writable directory of your account, make it executable and start it:

```sh
chmod +x Junto-<version>-x86_64.AppImage
./Junto-<version>-x86_64.AppImage
```

## Data and updates

The server book and the device keys are in `~/.local/share/junto` (or `$XDG_DATA_HOME/junto`, ADR 0710). A package never writes there.

A configured build checks for a new version at start and periodically. Click Update on its notice or Install update in Settings, Version. Junto verifies the signed release information, its date and the download, then replaces the directory or the AppImage file in one step. The AppImage keeps its file name. The update keeps the replaced version as an archive in a hidden directory beside the directory or the AppImage, named after it: `.junto.junto-update` for the directory `junto`, `.<file name>.junto-update` for an AppImage. The directory that holds them must stay writable. The first update also downloads the installed version from the release source for that purpose; when the source does not have it, the update is still installed and Version states that no previous version is kept. Quit and reopen Junto afterwards.

## Recovery

If the new version cannot start, quit every instance of Junto and run the recovery program kept beside the application, with the absolute path of the directory or of the AppImage file:

```sh
~/.local/opt/.junto.junto-update/record/previous/recover rollback ~/.local/opt/junto
```

It does not run the current program. It verifies the kept release signatures and archive bytes and puts the previous version back. It needs no network connection. It restores the program only; an older client may refuse a newer data-file format. If verification refuses recovery, keep the `.junto-update` directory and obtain an authentic package from an independently verified source.

An interruption can leave `stage-<random>` directories and `download-<random>` files in the `.junto-update` directory. Keep a staging directory until the installed program and the record have been checked: it can contain the only recovery copy.
