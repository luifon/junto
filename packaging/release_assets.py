"""Collects the files of one release for its GitHub release and writes their checksums (ADR 0727)."""
import argparse
import base64
import hashlib
import json
import os
import pathlib
import re
import shutil
import sys
import tempfile

REPOSITORY = "luifon/junto"
VERSION = re.compile(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)")
TARGETS = ("macos-bundle-aarch64", "linux-directory-x86_64", "linux-appimage-x86_64", "windows-directory-x86_64")
FIRST_INSTALL = (
    "Junto-{v}-macos-aarch64.dmg",
    "Junto-{v}-macos-aarch64.zip",
    "Junto-{v}-windows-x86_64-setup.exe",
    "Junto-{v}-windows-x86_64.zip",
    "Junto-{v}-x86_64.AppImage",
    "junto-{v}-linux-x86_64.tar.gz",
    "junto_{v}_amd64.deb",
)
SYMBOLS = "Junto-{v}-windows-x86_64.pdb"
CHECKSUMS = "SHA256SUMS"
CHUNK = 1024 * 1024


class AssetError(Exception):
    pass


def latest_name(target):
    return f"latest-{target}.json"


def release_name(version, target):
    return f"release-{version}-{target}.json"


def archive_name(version, target):
    return f"junto-{version}-{target}.junto"


def expected_names(version):
    """Every file of the release of `version`: the first-install files, the symbols file and the
    update files of each target."""
    names = [template.format(v=version) for template in FIRST_INSTALL] + [SYMBOLS.format(v=version)]
    for target in TARGETS:
        names += [latest_name(target), release_name(version, target), archive_name(version, target)]
    return names


def envelope(path):
    try:
        value = json.loads(path.read_bytes())
        return value if isinstance(value, dict) else {}
    except ValueError as error:
        raise AssetError(f"not release information: {path}: {error}") from error


def payload(path):
    """The release payload of a release information file, without a check of its signatures."""
    try:
        return json.loads(base64.b64decode(envelope(path)["payload"], validate=True))
    except (ValueError, KeyError, TypeError) as error:
        raise AssetError(f"not release information: {path}: {error}") from error


def signed_release(path):
    """The part of release information that the release keys sign; a freshness renewal keeps it."""
    value = envelope(path)
    return value.get("payload"), value.get("signatures")


def check_update_files(directory, version):
    """The latest file and the release file of each target hold the same signed release, and its
    payload names this version, this target and its archive. A renewed latest file differs only in
    its freshness statement. The signatures are checked by the release tool and the package checks
    before the files reach this directory."""
    problems = []
    for target in TARGETS:
        latest, release = directory / latest_name(target), directory / release_name(version, target)
        if signed_release(latest) != signed_release(release):
            problems.append(f"{latest.name} holds another signed release than {release.name}")
            continue
        stated = payload(release)
        system, package, architecture = target.split("-")
        wanted = {"version": version, "system": system, "package": package, "architecture": architecture,
                  "archive": archive_name(version, target)}
        wrong = sorted(key for key, value in wanted.items() if stated.get(key) != value)
        if wrong:
            problems.append(f"{release.name} states another {', '.join(wrong)}")
        elif stated.get("size") != (directory / archive_name(version, target)).stat().st_size:
            problems.append(f"{release.name} states another size than {archive_name(version, target)}")
    return problems


def read_release(directory, version):
    directory = pathlib.Path(directory)
    if VERSION.fullmatch(version) is None:
        raise AssetError(f"not a version of three numbers: {version}")
    if not directory.is_dir():
        raise AssetError(f"the release directory does not exist: {directory}")
    expected = expected_names(version)
    problems = []
    for path in sorted(directory.iterdir()):
        if path.is_symlink() or not path.is_file():
            problems.append(f"not a regular file: {path}")
        elif path.name not in expected:
            problems.append(f"unknown file: {path}")
    problems += [f"missing file: {directory / name}" for name in expected if not (directory / name).is_file()]
    if problems:
        raise AssetError("\n".join(problems))
    problems = check_update_files(directory, version)
    if problems:
        raise AssetError("\n".join(problems))
    return [directory / name for name in expected]


def copy(source, destination):
    """Copies the bytes of a file and returns its SHA-256."""
    digest = hashlib.sha256()
    with open(source, "rb") as reader, open(destination, "xb") as writer:
        while chunk := reader.read(CHUNK):
            digest.update(chunk)
            writer.write(chunk)
    return digest.hexdigest()


def checksums(digests):
    """The lines of `sha256sum` for every file except the latest files, which a freshness renewal
    replaces (ADR 0724)."""
    return "".join(f"{digests[name]}  {name}\n" for name in sorted(digests) if not name.startswith("latest-"))


def build(directory, version, output):
    """Writes the files of the release and SHA256SUMS into the new directory OUTPUT and returns
    each written file with its size."""
    output = pathlib.Path(output).absolute()
    if output.exists() or output.is_symlink():
        raise AssetError(f"the output directory exists: {output}")
    if not output.parent.is_dir():
        raise AssetError(f"the parent of the output directory does not exist: {output.parent}")
    files = read_release(directory, version)
    temporary = pathlib.Path(tempfile.mkdtemp(prefix=f".{output.name}.", dir=output.parent))
    try:
        digests = {path.name: copy(path, temporary / path.name) for path in files}
        (temporary / CHECKSUMS).write_text(checksums(digests), encoding="utf-8", newline="\n")
        temporary.chmod(0o755)
        os.rename(temporary, output)
    except BaseException:
        shutil.rmtree(temporary, ignore_errors=True)
        raise
    return [(path.name, path.stat().st_size) for path in sorted(output.iterdir())]


def quoted(text):
    return "'" + text.replace("'", "'\\''") + "'"


def publication(output, version, title, notes, prerelease=None):
    """The commands that publish the files as the GitHub release of the tag v<version>. A draft is
    not served, so every file is present when the release is published. `prerelease` names the
    title and the notes of the later promotion of a pre-release; the promotion changes no file."""
    tag = f"v{version}"
    lines = ["Publication (the files of a draft are not served before it is published):",
             f"gh release create {tag} --repo {REPOSITORY} --draft --verify-tag --title {quoted(title)} "
             f"--notes-file {notes} {output}/*",
             f"gh release edit {tag} --repo {REPOSITORY} --draft=false "
             + ("--prerelease" if prerelease else "--latest")]
    if prerelease:
        final_title, final_notes = prerelease
        lines += ["Promotion to the newest release, when the pre-release is accepted (no file changes):",
                  f"gh release edit {tag} --repo {REPOSITORY} --prerelease=false --latest --title {quoted(final_title)} "
                  f"--notes-file {final_notes}"]
    return lines + [f"Update source of the builds (JUNTO_UPDATE_SOURCE): https://github.com/{REPOSITORY}/releases/"]


def main(arguments=None):
    parser = argparse.ArgumentParser(description="Collects the files of one release for its GitHub release (ADR 0727).")
    parser.add_argument("version", help="the version of the release, three numbers")
    parser.add_argument("release", help="the directory with every file of the release from every system")
    parser.add_argument("output", help="the new output directory")
    parser.add_argument("--title", help="the title of the release; Junto <version> when absent")
    parser.add_argument("--notes", default="docs/releases/<version>.md", help="the notes file of the release")
    parser.add_argument("--prerelease", action="store_true",
                        help="publish a pre-release and print the command of its later promotion")
    parser.add_argument("--final-title", help="the title after the promotion; Junto <version> when absent")
    parser.add_argument("--final-notes", default="docs/releases/<version>.md", help="the notes after the promotion")
    options = parser.parse_args(arguments)
    try:
        files = build(options.release, options.version, options.output)
    except (AssetError, OSError) as error:
        print(f"release_assets.py: {error}", file=sys.stderr)
        return 1
    for name, size in files:
        print(f"{size:>14,}  {name}")
    named = lambda text: text.replace("<version>", options.version)
    final = (options.final_title or f"Junto {options.version}", named(options.final_notes))
    for line in publication(pathlib.Path(options.output).absolute(), options.version,
                            options.title or f"Junto {options.version}", named(options.notes),
                            final if options.prerelease else None):
        print(line)
    return 0


if __name__ == "__main__":
    sys.exit(main())
