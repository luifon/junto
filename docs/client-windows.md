# Install Junto on Windows

Junto runs on Windows 11 on x86-64 (ADR 0705). Obtain `Junto-<version>-windows-x86_64-setup.exe`, or the ZIP file `Junto-<version>-windows-x86_64.zip`, from the download page of the release site or from a source you have verified with whoever runs your server, and compare its SHA-256 with `SHA256SUMS` (`Get-FileHash -Algorithm SHA256 <file>` in PowerShell). Both hold one folder `Junto` (ADR 0755, ADR 0728); Junto needs no administrator rights and no other runtime.

## Install with the setup program

1. Start `Junto-<version>-windows-x86_64-setup.exe`. It installs Junto for your account into `%LOCALAPPDATA%\Programs\Junto`, adds Junto to the Start menu and to Settings, Apps, Installed apps, and starts Junto. It asks for no administrator rights. When Junto runs, it asks you to quit it first.
2. A setup program of a newer version, started the same way, replaces the installed version and keeps your data.

## Install from the ZIP file

1. Open the ZIP and extract the folder `Junto` into a folder of your account, for example `%LOCALAPPDATA%\Programs`. The folder that holds `Junto` must stay writable for updates.
2. Start `Junto\junto.exe`. Junto opens no console window; started from a terminal, it returns to the prompt at once. This way adds no Start menu entry and no entry in Settings, Apps.

## Taskbar

1. To start Junto from the taskbar, right-click its taskbar button while it runs and choose "Pin to taskbar". Junto names its own identity for the taskbar (ADR 0755), and at each start it gives that identity to a pin made from `junto.exe` in Explorer, also one made for an earlier version, so that the pin and the running Junto are one taskbar button. If such a pin still opens Junto as a second taskbar button, unpin it and pin the running Junto instead. The pin starts `junto.exe` without arguments, so with the default data directory, also when the running Junto was started with `--data-dir`.

The log of a running Junto is kept only when standard error is redirected, for example `junto.exe 2> "%TEMP%\junto.log"` in `cmd.exe`.

The project does not buy a code-signing certificate (`docs/scope.md`). At the first start of a downloaded setup program or copy, Microsoft Defender SmartScreen states that it protected your computer from an unrecognized app ("Windows protected your PC"). Choose "More info" and "Run anyway" only for a file from your verified source. With Smart App Control on, Windows does not run Junto (ADR 0704).

The server book and the device keys are in `%LOCALAPPDATA%\Junto` (ADR 0710).

## Log and error reports

Junto writes its log to `%LOCALAPPDATA%\Junto\logs\junto.log` (at most three files of 4 MiB: `junto.log`, `junto.log.1`, `junto.log.2`). When an error stops an action of Junto, it writes a report to `%LOCALAPPDATA%\Junto\panics`, continues, and names the report in a notice at the bottom of the window. Nothing of either is sent anywhere; send a report to whoever runs your server only if you choose to. `JUNTO_LOG_LEVEL` sets the level of the log; by default the log holds the information lines of Junto and the warnings of the libraries it uses. When a second instance of Junto runs on the same data directory, only the first writes the log. When Junto cannot open its data directory at the start, it writes the reason to `junto-start-failure.txt` in the temporary directory (`%TEMP%`) (ADR 0757).

A report of the Windows client names no function of Junto, because the package carries no symbols. Its section `frames` lists for each frame the address, the base of its module and the offset. The release publishes the symbols as `Junto-<version>-windows-x86_64.pdb`, for the version that the report states. Put `junto.exe` of that version and that file, renamed to `junto.pdb`, into one folder; `llvm-symbolizer` of LLVM (on any system) then resolves the offsets of the frames of `junto.exe`, the lines whose base is the base of `junto.exe`:

```sh
llvm-symbolizer --obj=junto.exe --relative-address 0x<offset> 0x<offset> ...
```

## Updates

A configured build checks for a new version at start and periodically. The server book shows the version of Junto below "Add server"; when a newer version exists, choose Update there, on the notice of the channel column, or Install update in Settings, Version. Junto verifies the signed release information, its date and the download, then replaces the files of the folder one by one while it runs. The update keeps the replaced version as an archive in the folder `.Junto.junto-update` beside `Junto`. The first update also downloads the installed version from the release source for that purpose; when the source does not have it, the update is still installed and Version states that no previous version is kept. Then choose Restart: Junto quits and starts the new version. The replaced program file is removed by the next update after it has ended.

## Recovery

If the new version cannot start, quit every instance of Junto, open a terminal and run the recovery program kept beside the folder, with the full path of the folder:

```bat
"%LOCALAPPDATA%\Programs\.Junto.junto-update\record\previous\recover.exe" rollback "%LOCALAPPDATA%\Programs\Junto"
```

It does not run the current program. It verifies the kept release signatures and archive bytes and puts the previous version back. It needs no network connection. It restores the program only; an older client may refuse a newer data-file format. If verification refuses recovery, keep `.Junto.junto-update` and obtain an authentic package from an independently verified source.

## Notifications

Junto registers the name and the picture of its notifications for your account at each start (ADR 0742): the key `HKEY_CURRENT_USER\Software\Classes\AppUserModelId\chat.junto.Junto` and the file `notification-icon.png` in its data directory. Windows shows the notifications of Junto with that name and picture.

## Remove

Quit Junto and unpin it from the taskbar (right-click the pin, "Unpin from taskbar"). Then choose Uninstall for Junto in Settings, Apps, Installed apps, or start `uninstall.exe --uninstall` in the folder `Junto` (also for a copy from the ZIP file). It asks whether to remove Junto, then whether to remove your data as well; the default keeps `%LOCALAPPDATA%\Junto` with the server book and the device keys. It removes the registration of the notifications, the Start menu entry, the entry in Settings, Apps, the folder `Junto` and the folder `.Junto.junto-update` beside it. `uninstall.exe --uninstall --quiet` asks nothing and keeps the data; `--uninstall --quiet --remove-data` also removes it.

If the folder is already deleted, remove the key of the notifications with `reg delete "HKCU\Software\Classes\AppUserModelId\chat.junto.Junto" /f`, and the entry of the installed programs with `reg delete "HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\Junto" /f`.
