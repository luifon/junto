"""Renews the freshness of the newest release in its retained files and, with --upload, replaces
the latest files of its GitHub release (ADR 0724)."""
import argparse
import base64
import datetime
import json
import pathlib
import subprocess
import sys
import time
import urllib.request

from release_assets import REPOSITORY, TARGETS, VERSION, archive_name, latest_name

# The release tool signs 30 days; a statement that ends sooner after a renewal is not one.
LEAST_VALIDITY = 29 * 24 * 60 * 60


class RenewalError(Exception):
    pass


def version_of(directory, target):
    """The version of the one release file of `target` in the retained directory."""
    found = [path.name[len("release-"):-len(f"-{target}.json")]
             for path in directory.glob(f"release-*-{target}.json")]
    found = [version for version in found if VERSION.fullmatch(version)]
    if len(found) != 1:
        raise RenewalError(f"{directory} holds {len(found)} release files of {target}; it must hold one")
    return found[0]


def expiry(path):
    """The end of the freshness statement of a release information file, in Unix seconds."""
    try:
        envelope = json.loads(path.read_bytes())
        return int(json.loads(base64.b64decode(envelope["freshness"]["payload"], validate=True))["expires"])
    except (ValueError, KeyError, TypeError) as error:
        raise RenewalError(f"no freshness statement in {path}: {error}") from error


def renew(tool, directory, secrets, run=subprocess.run, now=time.time):
    """Refreshes the latest file of every target with the release tool, which verifies the release
    and its archive with its compiled trust sets first. Returns the renewed files."""
    directory = pathlib.Path(directory)
    versions = {target: version_of(directory, target) for target in TARGETS}
    if len(set(versions.values())) != 1:
        raise RenewalError(f"the release files name more than one version: {versions}")
    missing = [path for target, version in versions.items()
               for path in (directory / latest_name(target), directory / archive_name(version, target))
               if not path.is_file()]
    if missing:
        raise RenewalError("\n".join(f"missing file: {path}" for path in missing))
    renewed = []
    for target, version in versions.items():
        latest = directory / latest_name(target)
        result = run([str(tool), "refresh", str(latest), str(directory / archive_name(version, target)), version,
                      *map(str, secrets)], capture_output=True, text=True, timeout=300)
        if result.returncode:
            raise RenewalError(f"the refresh of {latest.name} failed: {result.stderr.strip()}")
        ends = expiry(latest)
        if ends < now() + LEAST_VALIDITY:
            raise RenewalError(f"{latest.name} is fresh only until {date(ends)}")
        renewed.append((latest, version, ends))
    return renewed


def date(seconds):
    return datetime.datetime.fromtimestamp(seconds, datetime.timezone.utc).strftime("%Y-%m-%d %H:%M UTC")


def fetch(address):
    with urllib.request.urlopen(address, timeout=60) as response:
        return response.read()


def check_upload(tag, directory, run=subprocess.run):
    """Before any refresh: the tag is v<version> of the directory, and the GitHub command is
    signed in."""
    version = version_of(pathlib.Path(directory), TARGETS[0])
    if tag != f"v{version}":
        raise RenewalError(f"the tag {tag} does not name version {version} of {directory}")
    result = run(["gh", "auth", "status"], capture_output=True, text=True, timeout=60)
    if result.returncode:
        raise RenewalError(f"the GitHub command is not signed in: {result.stderr.strip()}")


def upload(tag, files, run=subprocess.run, fetch=fetch):
    """Replaces the latest files of the release `tag`, then reads each one back from the address of
    the newest release, as clients read it."""
    result = run(["gh", "release", "upload", tag, *map(str, files), "--repo", REPOSITORY, "--clobber"],
                 capture_output=True, text=True, timeout=600)
    if result.returncode:
        raise RenewalError(f"the upload to {tag} failed: {result.stderr.strip()}")
    differ = [path.name for path in files
              if fetch(f"https://github.com/{REPOSITORY}/releases/latest/download/{path.name}") != path.read_bytes()]
    if differ:
        raise RenewalError(f"the newest release does not serve the renewed {', '.join(differ)}; "
                           f"check that {tag} is the newest release")


def main(arguments=None):
    parser = argparse.ArgumentParser(description="Renews the freshness of the newest release (ADR 0724).")
    parser.add_argument("tool", help="junto-release of the release, with the compiled trust sets")
    parser.add_argument("directory", help="the retained files of the newest release, as release_assets.py wrote them")
    parser.add_argument("secrets", nargs="+", help="the freshness secret files")
    parser.add_argument("--upload", metavar="TAG", help="replace the latest files of this GitHub release")
    options = parser.parse_args(arguments)
    try:
        if options.upload:
            check_upload(options.upload, options.directory)
        renewed = renew(options.tool, options.directory, options.secrets)
        for path, version, ends in renewed:
            print(f"{path.name}: version {version}, fresh until {date(ends)}")
        if options.upload:
            upload(options.upload, [path for path, _, _ in renewed])
            print(f"The newest release {options.upload} serves the renewed files.")
    except (RenewalError, OSError, subprocess.SubprocessError) as error:
        print(f"renew_freshness.py: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
