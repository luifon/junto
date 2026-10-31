# 0727. The GitHub release and the release notes

## Status

Accepted

Before the first release this record described a static release site that `packaging/site.py` wrote, with the update source in its directory `updates/`. The site was removed when the GitHub releases of the public repository became the update source and the download location (ADR 0724): a second host would have held the same files, and the client follows the layout of GitHub releases.

The unit tests of `packaging/test_release_assets.py` ran on Linux and on macOS. Not run: a publication to GitHub.

## Context

ADR 0724 makes the GitHub releases of `https://github.com/luifon/junto` the update source: the release of version V has the tag `v<V>` and holds the update files of every target, and the latest address serves the latest files of the newest release that is neither a draft nor a pre-release. The release commands write the files of one release into a new directory on each build system (`packaging/README.md`). A person who installs Junto for the first time needs the first-install files, their checksums, the steps of the first start of an application without a paid signature (ADR 0704), and the notes of the release.

## Decision

- `packaging/release_assets.py <version> <release directory> <new output directory> [--title <title>] [--notes <file>] [--prerelease [--final-title <title>] [--final-notes <file>]]` collects the files of one release. The release directory holds every file of the release from every system, as the release commands write them.
- The files of a release are the first-install files `Junto-<version>-macos-aarch64.dmg`, `Junto-<version>-macos-aarch64.zip`, `Junto-<version>-windows-x86_64-setup.exe`, `Junto-<version>-windows-x86_64.zip`, `Junto-<version>-x86_64.AppImage`, `junto-<version>-linux-x86_64.tar.gz` and `junto_<version>_amd64.deb`; the symbols file `Junto-<version>-windows-x86_64.pdb` (ADR 0755, ADR 0757); and for each of the four targets `macos-bundle-aarch64`, `linux-directory-x86_64`, `linux-appimage-x86_64` and `windows-directory-x86_64` the files `latest-<target>.json`, `release-<version>-<target>.json` and `junto-<version>-<target>.junto`. Every release holds the latest files of every target, because the latest address serves only the files of the newest release.
- The command stops with an error that names each file: on a missing file, an unknown file, a link or a directory, on a latest file whose signed release (payload and release signatures) differs from the release file of its target, and on release information whose payload names another version, target or archive name, or another size than the archive. It does not verify signatures: the release commands and the package checks of ADR 0724, ADR 0754 and ADR 0755 do that before the files reach the directory.
- The output directory must not exist. The command copies every file byte for byte into a temporary directory beside it, writes `SHA256SUMS` in the format of `sha256sum`, sorted by file name, and renames the directory into place, so a failure leaves no output directory. `SHA256SUMS` lists every file except the latest files, which a freshness renewal replaces (ADR 0724). The command never writes into the release directory.
- The command prints every file with its size and the commands of the publication under the tag `v<version>`, the only tag at which clients find the files of the version (ADR 0724): `gh release create v<version> --draft --verify-tag` with every file, the title and the notes, then `gh release edit v<version> --draft=false --latest`. The files of a draft are not served, so the latest files become available together with every archive that they name. With `--prerelease` the release is published as a pre-release, which the latest address does not serve, and the command also prints its promotion: `gh release edit v<version> --prerelease=false --latest` with the final title and notes. The promotion changes no file, so the files of a test release become the release of the version that they state.
- The command uploads nothing. The operator publishes; the output directory is kept as the retained files of the release, which the freshness renewal of ADR 0724 reads.
- The notes of each version are `docs/releases/<version>.md`, the text of the GitHub release. They name the server install document by its path in the repository.

## Consequences

- A release needs every first-install file, the symbols file and the update files of the four targets. A new first-install file or a new target is a change of `release_assets.py` and of this record.
- The first-install steps and the checksum commands are in the notes of each release and in `docs/client-macos.md`, `docs/client-linux.md` and `docs/client-windows.md`, because GitHub shows the notes beside the files.
- Not checked: a publication to GitHub with these commands, and the page of a release on GitHub.
