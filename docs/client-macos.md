# Install Junto on macOS

Obtain `Junto-<version>-macos-aarch64.dmg`, or the ZIP file `Junto-<version>-macos-aarch64.zip`, from the download page of the release site or from a source you have verified with whoever runs your server, and compare its SHA-256 with `SHA256SUMS` (`shasum -a 256 <file>`). The tested system is macOS 26 on Apple Silicon; older versions are not blocked and are not tested (ADR 0705). Open the disk image and drag Junto onto the Applications link, or open the ZIP in Finder and move `Junto.app` into a writable directory of your account, such as your own Applications directory (ADR 0728). Open it there. The server book and device keys are stored separately under `~/Library/Application Support/Junto`.

On macOS the first launch of a downloaded copy is blocked. The dialog has no button that opens the application. Open System Settings, Privacy & Security, and choose "Open Anyway" only for the copy obtained from your verified source. The self-signed certificate does not remove this first-launch block. Do not install that certificate or mark it as trusted.

Junto asks macOS for the notifications at its first start, and for the microphone, the camera and the screen when you first use them; Input Monitoring when push to talk is set; macOS asks for the Downloads folder and the local network at their first use. Settings, Permissions lists each permission with its state and, when macOS does not allow it, the button that opens its page of System Settings (ADR 0708). If Junto is turned on there but not allowed, an entry of an earlier copy with another signature remains: select Junto, remove it with the minus button, and allow it again. Permission identity depends on keeping the bundle identifier and signing certificate unchanged; the certificate is separate from both update-key roles. 0.1.0 is the first version signed with the permanent certificate: a copy of a test version had other signatures, so macOS asks once more.

A configured build checks at startup and periodically. The server book shows the version of Junto below "Add server"; when a newer version exists, choose Update there, on the notice of the channel column, or Install update in Settings, Version. Junto verifies release authorization, the date of the update information and the download, then the candidate's code-signing identity. An update keeps the replaced version as an archive in the hidden directory `.Junto.app.junto-update` beside `Junto.app`, not as another application. The first update also downloads the installed version from the release source for that purpose; when the source does not have it, the update is still installed and Version states that no previous version is kept. Then choose Restart: Junto leaves its call, quits and opens the new version; an update never restarts by itself.

Version states when no source is configured or when verification or the date of the update information prevents an update. Unrecognized signing keys do not produce a download link or installation action. A build marked "test configuration" is not a product release.

Junto writes its log to `logs/junto.log` in `~/Library/Application Support/Junto` (at most three files of 4 MiB: `junto.log`, `junto.log.1`, `junto.log.2`). When an error stops an action of Junto, it writes a report to `panics/` in the same directory, continues, and names the report in a notice at the bottom of the window. Nothing of either is sent anywhere; send a report to whoever runs your server only if you choose to. `JUNTO_LOG_LEVEL` sets the level of the log; by default the log holds the information lines of Junto and the warnings of the libraries it uses. When a second instance of Junto runs on the same data directory, only the first writes the log. When Junto cannot open its data directory at the start, it writes the reason to `junto-start-failure.txt` in the temporary directory (`$TMPDIR`) (ADR 0757).

If the new application cannot start, recover it as follows:

1. Quit every running instance of Junto.
2. In Terminal, run the recovery program kept beside the application, replacing both paths below with the paths of your installation:

```sh
"/path/to/.Junto.app.junto-update/record/previous/recover" rollback "/path/to/Junto.app"
```

3. When the command succeeds, open `Junto.app` normally.

The command does not run the broken application. It verifies the kept release signatures and archive bytes, extracts privately, verifies the restored bundle's certificate and version, then replaces the application. It needs no network connection, private signing key or Python installation. It can restore an application with a damaged current code signature. Recovery restores application files only; an older client may refuse a newer data-file format instead of downgrading it.

If verification refuses recovery, keep `.Junto.app.junto-update` and ask the server operator for an authentic installer from an independently verified source. Do not unpack and execute an unverified recovery archive. The command does not detect other running instances; quitting them is required.

An interruption can leave `stage-<random>` directories and `download-<random>` files in `.Junto.app.junto-update`. Keep a staging directory until the installed application and the record have been checked: it can contain the only recovery copy. Updates require writable space and never elevate privileges.

GUI startup of this bundle, a downloaded copy with quarantine, a clean-system installation, real notifications and capture permissions after updating remain untested. Linux and Windows have their own instructions: [Linux](client-linux.md), [Windows](client-windows.md). [Release operator instructions](../packaging/README.md) describe certificate and key custody and release creation; no release command uploads anything.
