# 0755. Windows package

## Status

Accepted

The package is built and checked by the Windows job of CI only (ADR 0753, ADR 0905); no Windows machine outside CI ran it.

## Context

The client runs on Windows 11 on x86-64 (ADR 0705). The project buys no code-signing certificate (`docs/scope.md`, "Not planned"), so Windows treats every file of the package as coming from an unknown publisher. The client updates itself from signed releases (ADR 0724, ADR 0726). The requirements of a Windows package are an installation without administrator rights, the replacement of the program while it runs, a rollback, and a statement of what Windows shows to the person.

Windows lets a running program file be renamed and moved on its volume; it does not let the file be deleted or written. It refuses to rename a folder that holds a running program. Files that a browser downloads carry a mark of their origin ("Mark of the Web"), and Explorer gives the mark to the files that it extracts from a marked ZIP file.

The options:

- An MSI file: it can install for one account, but its upgrade cannot replace a running program without closing it, and its rollback exists only during the installation transaction, not as a later return to the previous version. It would need the WiX tool set in CI.
- An MSIX package: Windows installs it only with a signature from a certificate that the machine trusts. A self-signed certificate must first be installed into the machine's store of trusted people, which needs administrator rights.
- A setup program (NSIS, Inno Setup): it is one more unsigned program, and the update of the client would still be its own; it would only copy files.

## Decision

- The package is a ZIP file `Junto-<version>-windows-x86_64.zip` with one folder `Junto`: `junto.exe`, `junto-recover.exe`, the description `junto-package.json` (format 1, version, system `windows`, package `directory`, architecture), and `licences\` with the licence of Junto and the licences of the fonts. The program has the static C runtime (ADR 0753) and no library of its own.
- The person extracts the folder into a folder of their account, such as `%LOCALAPPDATA%\Programs`. No step needs administrator rights, writes to the registry or installs a service. The data directory is `%LOCALAPPDATA%\Junto` (ADR 0710).
- The update archive holds the folder `Junto`. The installer replaces the entries of the installed folder one by one: each old entry moves into the staging directory, then each new entry moves into the folder. A failure moves the entries back; a failure of that return is reported, and the staging directory is kept because it holds the application.
- The replaced running program stays in the staging directory until it ends. A mark beside that directory lets the next update or rollback remove it (ADR 0726).
- Rollback: `.Junto.junto-update\record\previous\recover.exe rollback <folder>` beside the folder, with every instance closed.
- What Windows shows: at the first start of a program that was extracted from a downloaded ZIP file, Microsoft Defender SmartScreen states that it protected the computer from an unrecognized app; "More info" shows "Run anyway". Files that the client downloads and writes for an update carry no mark of their origin. With Smart App Control on, Windows does not run the program (ADR 0704).

Checks (`packaging/check_windows.py`, in the Windows job of CI):

- The ZIP of the client build: its entries and no library; the icon group 1 of `junto.exe` with the rendering of the icon at each size (ADR 0756); extraction into a folder; `junto.exe --version`; the first frame with `--check-startup`.
- With test programs: an update from a local source while the installed program runs, the requests of the update, the new program in place, the mark of the staging directory; then the rollback with the recovery program, a changed archive and a second rollback refused, and no staging directory left.

## Consequences

- The person creates a Start menu or taskbar entry; the package does not. The program holds the application icon as its icon group 1 (ADR 0756): Explorer, an entry that the person creates and the windows of the program show it.
- An uninstallation is the removal of the folder and of `.Junto.junto-update` beside it; the data directory stays until the person removes it.
- The replacement of the entries is not one step. Both lists of entries are read before the first move, and every return move is tried after a failure. When a return move also fails, the folder holds entries of both versions and the staging directory, which is kept, holds the rest; the error names it. The recovery program then can refuse, because the folder may have no description or one that differs from the record: the entries are moved back from the staging directory by hand. A crash between two moves leaves the same state.
- The SmartScreen warning appears at the first start of each new download, not after an update by the client.
