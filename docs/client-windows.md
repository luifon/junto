# Install Junto on Windows

Junto runs on Windows 11 on x86-64 (ADR 0705). Obtain `Junto-<version>-windows-x86_64.zip` from a source you have verified with whoever runs your server. The package is one folder `Junto` (ADR 0755); it needs no administrator rights and no other runtime.

## Install

1. Open the ZIP and extract the folder `Junto` into a folder of your account, for example `%LOCALAPPDATA%\Programs`. The folder that holds `Junto` must stay writable for updates.
2. Start `Junto\junto.exe`. To start it from the Start menu or the taskbar, pin it there.

The project does not buy a code-signing certificate (`docs/scope.md`). At the first start of a downloaded copy, Microsoft Defender SmartScreen states that it protected your computer from an unrecognized app. Choose "More info" and "Run anyway" only for a copy from your verified source. With Smart App Control on, Windows does not run Junto (ADR 0704).

The server book and the device keys are in `%LOCALAPPDATA%\Junto` (ADR 0710).

## Log and error reports

Junto writes its log to `%LOCALAPPDATA%\Junto\logs\junto.log` (at most three files of 4 MiB: `junto.log`, `junto.log.1`, `junto.log.2`). When an error stops an action of Junto, it writes a report to `%LOCALAPPDATA%\Junto\panics`, continues, and names the report in a notice at the bottom of the window. Nothing of either is sent anywhere; send a report to whoever runs your server only if you choose to. `JUNTO_LOG_LEVEL` sets the level of the log; by default the log holds the information lines of Junto and the warnings of the libraries it uses. When a second instance of Junto runs on the same data directory, only the first writes the log. When Junto cannot open its data directory at the start, it writes the reason to `junto-start-failure.txt` in the temporary directory (`%TEMP%`) (ADR 0757).

A report of the Windows client names no function of Junto, because the package carries no symbols. Its section `frames` lists for each frame the address, the base of its module and the offset. The release publishes the symbols as `Junto-<version>-windows-x86_64.pdb`, for the version that the report states. Put `junto.exe` of that version and that file, renamed to `junto.pdb`, into one folder; `llvm-symbolizer` of LLVM (on any system) then resolves the offsets of the frames of `junto.exe`, the lines whose base is the base of `junto.exe`:

```sh
llvm-symbolizer --obj=junto.exe --relative-address 0x<offset> 0x<offset> ...
```

## Updates

A configured build checks for a new version at start and periodically. Click Update on its notice or Install update in Settings, Version. Junto verifies the signed release information, its date and the download, then replaces the files of the folder one by one while it runs. The update keeps the replaced version as an archive in the folder `.Junto.junto-update` beside `Junto`. The first update also downloads the installed version from the release source for that purpose; when the source does not have it, the update is still installed and Version states that no previous version is kept. Quit and reopen Junto afterwards. The replaced program file is removed by the next update after it has ended.

## Recovery

If the new version cannot start, quit every instance of Junto, open a terminal and run the recovery program kept beside the folder, with the full path of the folder:

```bat
"%LOCALAPPDATA%\Programs\.Junto.junto-update\record\previous\recover.exe" rollback "%LOCALAPPDATA%\Programs\Junto"
```

It does not run the current program. It verifies the kept release signatures and archive bytes and puts the previous version back. It needs no network connection. It restores the program only; an older client may refuse a newer data-file format. If verification refuses recovery, keep `.Junto.junto-update` and obtain an authentic package from an independently verified source.

## Remove

Quit Junto and delete the folder `Junto` and the folder `.Junto.junto-update` beside it. Delete `%LOCALAPPDATA%\Junto` only if you also want to remove the server book and the device keys.
