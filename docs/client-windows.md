# Install Junto on Windows

Junto runs on Windows 11 on x86-64 (ADR 0705). Obtain `Junto-<version>-windows-x86_64.zip` from a source you have verified with whoever runs your server. The package is one folder `Junto` (ADR 0755); it needs no administrator rights and no other runtime.

## Install

1. Open the ZIP and extract the folder `Junto` into a folder of your account, for example `%LOCALAPPDATA%\Programs`. The folder that holds `Junto` must stay writable for updates.
2. Start `Junto\junto.exe`. Junto opens no console window; started from a terminal, it returns to the prompt at once.
3. To start Junto from the taskbar, right-click its taskbar button while it runs and choose "Pin to taskbar". Junto names its own identity for the taskbar (ADR 0755), so a pin made from `junto.exe` in Explorer, also one made for an earlier version, opens Junto as a second taskbar button: unpin it and pin the running Junto instead. The pin starts `junto.exe` without arguments, so with the default data directory, also when the running Junto was started with `--data-dir`.

The log of a running Junto is kept only when standard error is redirected, for example `junto.exe 2> "%TEMP%\junto.log"` in `cmd.exe`.

The project does not buy a code-signing certificate (`docs/scope.md`). At the first start of a downloaded copy, Microsoft Defender SmartScreen states that it protected your computer from an unrecognized app. Choose "More info" and "Run anyway" only for a copy from your verified source. With Smart App Control on, Windows does not run Junto (ADR 0704).

The server book and the device keys are in `%LOCALAPPDATA%\Junto` (ADR 0710).

## Updates

A configured build checks for a new version at start and periodically. Click Update on its notice or Install update in Settings, Version. Junto verifies the signed release information, its date and the download, then replaces the files of the folder one by one while it runs. The update keeps the replaced version as an archive in the folder `.Junto.junto-update` beside `Junto`. The first update also downloads the installed version from the release source for that purpose; when the source does not have it, the update is still installed and Version states that no previous version is kept. Quit and reopen Junto afterwards. The replaced program file is removed by the next update after it has ended.

## Recovery

If the new version cannot start, quit every instance of Junto, open a terminal and run the recovery program kept beside the folder, with the full path of the folder:

```bat
"%LOCALAPPDATA%\Programs\.Junto.junto-update\record\previous\recover.exe" rollback "%LOCALAPPDATA%\Programs\Junto"
```

It does not run the current program. It verifies the kept release signatures and archive bytes and puts the previous version back. It needs no network connection. It restores the program only; an older client may refuse a newer data-file format. If verification refuses recovery, keep `.Junto.junto-update` and obtain an authentic package from an independently verified source.

## Notifications

Junto registers the name and the picture of its notifications for your account at each start (ADR 0742): the key `HKEY_CURRENT_USER\Software\Classes\AppUserModelId\chat.junto.Junto` and the file `notification-icon.png` in its data directory. Windows shows the notifications of Junto with that name and picture.

## Remove

Quit Junto, unpin it from the taskbar (right-click the pin, "Unpin from taskbar"), remove the registration of its notifications, then delete the folder `Junto` and the folder `.Junto.junto-update` beside it:

```bat
"%LOCALAPPDATA%\Programs\Junto\junto-recover.exe" unregister
```

If the folder is already deleted, remove the key with `reg delete "HKCU\Software\Classes\AppUserModelId\chat.junto.Junto" /f`. Delete `%LOCALAPPDATA%\Junto` only if you also want to remove the server book and the device keys.
