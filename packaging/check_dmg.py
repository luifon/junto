#!/usr/bin/env python3
"""Checks the disk image of a macOS release on macOS (ADR 0728): its checksum, its signature
by the project certificate, and, mounted read-only without a Finder window, the volume that
holds the signed `Junto.app` with the designated requirement of the bundle and a link to the
Applications folder, and nothing else. The image is detached also after a failure."""
import os
import pathlib
import subprocess
import sys
import tempfile

from check_signing import requirement


def run(*arguments):
    result = subprocess.run([str(argument) for argument in arguments], capture_output=True, text=True,
                            timeout=300)
    if result.returncode != 0:
        raise AssertionError(f"{' '.join(map(str, arguments))} failed: {result.stdout}{result.stderr}")
    return result.stdout


def check(image, bundle_requirement, pin):
    image = pathlib.Path(image)
    run("/usr/bin/hdiutil", "verify", image)
    run("/usr/bin/codesign", "--verify", "--strict", image)
    signed = requirement(image).lower()
    if f'certificate leaf = h"{pin.lower()}"' not in signed:
        raise AssertionError(f"the image is not signed by the project certificate: {signed}")
    with tempfile.TemporaryDirectory(prefix="junto-dmg-") as temporary:
        mount = pathlib.Path(temporary) / "volume"
        mount.mkdir()
        run("/usr/bin/hdiutil", "attach", "-nobrowse", "-readonly", "-noautoopen", "-mountpoint", mount, image)
        try:
            names = sorted(path.name for path in mount.iterdir() if not path.name.startswith("."))
            if names != ["Applications", "Junto.app"]:
                raise AssertionError(f"the volume holds {names}")
            if os.readlink(mount / "Applications") != "/Applications":
                raise AssertionError("the link does not lead to /Applications")
            run("/usr/bin/codesign", "--verify", "--deep", "--strict", mount / "Junto.app")
            if requirement(mount / "Junto.app") != bundle_requirement:
                raise AssertionError("the bundle of the image has another designated requirement")
        finally:
            run("/usr/bin/hdiutil", "detach", mount)
    print(f"    {image.name}: signed, verified, Junto.app and the Applications link", flush=True)


if __name__ == "__main__":
    if len(sys.argv) != 4:
        sys.exit("usage: packaging/check_dmg.py <image.dmg> <Junto.app of the release> <certificate fingerprint>")
    check(sys.argv[1], requirement(pathlib.Path(sys.argv[2])), sys.argv[3])
