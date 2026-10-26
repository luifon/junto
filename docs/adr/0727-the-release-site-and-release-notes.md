# 0727. The release site and the release notes

## Status

Accepted

The unit tests of `packaging/test_site.py` ran on Linux. Not run: an upload to a real host, a page in a browser, and the update of a client from the `updates/` directory of a written site.

## Context

ADR 0704 makes the release page of the project the one source of client builds. ADR 0724 defines the update source: one HTTPS base address, `JUNTO_UPDATE_SOURCE`, that every build compiles, with the archive, the release information of every version and one latest file per target. The release commands write the files of one release into a new directory each time (`packaging/README.md`), and `docs/release.md` requires that a latest file is replaced only after the archive that it names is present. A person who installs Junto for the first time needs the first-install files of the newest version, their checksums, the steps of the first start of an application without a paid signature (ADR 0704), and the notes of the release. A GitHub release of the tag needs the same files and notes.

## Decision

- `packaging/site.py <releases directory> <notes directory> <new output directory> --base <address> --server-install <address>` writes the static files of the release site. The releases directory holds one directory per version, named with three decimal numbers, with every file of that release from every system as the release commands write them. Other directories, such as those of the test versions `0.1.0-test.1`, are ignored.
- The command stops with an error on a file of a version directory that it does not know, on a link or a directory in a version directory, on a latest file, release file or archive of a target without the release file and the archive of the same version and target beside it, on a version without notes, and when the newest version lacks a first-install file, the symbols file or a latest file. The error names each file.
- The first-install files of a version are `Junto-<version>-macos-aarch64.dmg`, `Junto-<version>-macos-aarch64.zip`, `Junto-<version>-windows-x86_64-setup.exe`, `Junto-<version>-windows-x86_64.zip`, `Junto-<version>-x86_64.AppImage`, `junto-<version>-linux-x86_64.tar.gz` and `junto_<version>_amd64.deb`. The symbols file `Junto-<version>-windows-x86_64.pdb` (ADR 0755, ADR 0757) is published with them, as another file.
- The layout of the site:
  - `index.html`: the download page of the newest version. One section per system with each first-install file, its size and its SHA-256, the install steps of each file, the first-start steps (macOS "Open Anyway"; Windows SmartScreen "More info" and "Run anyway", Smart App Control off), the checksum command of each system, the symbols file as another file, a link to the notes of the newest version, a list of every version with links to its notes and its files, and a link to the server install document.
  - `notes/<version>.html` for every version: the notes, and the files of that version with their sizes and SHA-256.
  - `download/<version>/` for every version: the first-install files and the symbols file that the version directory holds, and `SHA256SUMS` in the format of `sha256sum`, sorted by file name, over every file of the directory.
  - `updates/`: the update source. It holds the archives and the release files of every version and, for each target, the latest file of the highest version directory that holds one for that target. A target that a newer version does not have keeps the latest file of the older version.
- The update source of the clients is `<base>updates/`. Every build of a release takes that address as `JUNTO_UPDATE_SOURCE`. The address is an argument of the build and of the command; it is never written in a tracked file.
- The command copies every file byte for byte. It reads only the names of the signed files and does not change them, so the signatures of ADR 0724 stay valid. It never writes into the releases directory.
- The output directory must not exist. The command writes into a temporary directory beside it and renames that directory into place, so a failure leaves no output directory.
- The command prints every written file with its size and, as the last lines, the order of upload: every file except `updates/latest-*.json` first, then the latest files. A client therefore never reads a latest file that names an archive that is not yet on the site.
- The pages load nothing from another place. Each page holds its CSS in one `<style>` element and uses the fonts of the system. The download page holds one inline script; a notes page holds none. There is no external script, stylesheet, font, image or analytics. A Content-Security-Policy meta element allows `'self'` and the SHA-256 hashes of the inline style and the inline script (`script-src 'none'` on a notes page), and no `'unsafe-inline'`. The pages have no `style` attributes, which a hash does not allow.
- The visitor's system first: the inline script reads `navigator.userAgentData.platform` when it exists, else `navigator.platform` and `navigator.userAgent`. It moves the section of macOS, Windows or Linux to the top and marks it. Android, iOS and ChromeOS select no section. Without the script, the order is macOS, Windows, Linux, and the page is complete.
- The notes of each version are the file `docs/releases/<version>.md`. The site renders it, and the GitHub release of the tag uses the same file as its text. The renderer of `site.py` knows headings `#` to `###`, paragraphs, lists of one level, inline code, `**bold**`, links and fenced code blocks. It escapes every text from a file, also HTML in the Markdown and file names. A link is written as a link only for an `https:` address with a host or a relative address; any other scheme, an address that starts with `//`, and an address with a space, a control character or a backslash are written as text.
- The notes name the server install document by its path in the repository, not by a link, because a relative link of the repository does not exist on the site. The download page links to the address given with `--server-install`.

The tests (`packaging/test_site.py`) build a releases directory with versions 0.1.0 and 0.1.1 and the update files of the four targets, and check:

- The download page offers the files of the newest version with their real SHA-256 and size, the steps of the first start and the checksum commands.
- `SHA256SUMS` of each version verifies against the copied files, also with `sha256sum -c` where the program exists.
- `updates/` holds the archives and the release files of both versions; each latest file is the byte copy of the newest one; the target that only 0.1.0 has keeps the latest file of 0.1.0; a directory that is not a version is ignored; the releases directory is not changed.
- The notes render, HTML in the notes appears as text, and `javascript:`, `data:` and `//` links are not links.
- No page has an external script, stylesheet, font, `@import`, `url(`, `src` or `style` attribute; the hashes of the policy equal the inline style and script; the visitor-system script is inline.
- The command refuses an existing output directory, a version without notes, a missing first-install file of the newest version (and names each), a newest version without a latest file, a latest file without its archive, an unknown file, and an address that is not HTTPS or does not end in `/`. A failure while writing leaves no output directory and no temporary directory.

## Consequences

- One static host serves the download page and the update source. The host needs no program; it must serve HTTPS at the address that the builds compiled and must not redirect, because the client follows no redirect (ADR 0724).
- Changing the base address later does not move the clients that are already installed (`docs/release.md`). The site therefore keeps every earlier version, and the releases directory that the command reads must hold every published version.
- A release without a Debian package, a setup program or a disk image cannot be the newest version of the site. A new first-install file or a new target is a change of `site.py` and of this record.
- The command does not check the signatures or the content of a file. The release commands and the package checks of ADR 0724, ADR 0754 and ADR 0755 do that before the files reach the releases directory.
- The order of upload is printed, not run: the command uploads nothing, and the operator performs the two steps with the tool of the host.
- Not checked: a real host and its HTTPS, the page in a browser (the policy, the layout and the script that moves a section), and an update of a client from a written site.
