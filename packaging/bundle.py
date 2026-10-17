#!/usr/bin/env python3
"""Assemble a macOS application from the release binary; no application is launched."""
import os
import pathlib
import plistlib
import re
import shutil
import sys
import tomllib

import icons


def assemble(binary, destination, version, recovery=None, *, icon_tool):
    """Writes `destination`; `icon_tool` is the program `junto-icon` that writes the icon file."""
    binary, destination = pathlib.Path(binary), pathlib.Path(destination)
    if not re.fullmatch(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)", version):
        raise ValueError("a release version has three decimal numbers")
    if not binary.is_file() or binary.is_symlink():
        raise ValueError("the binary must be a regular file")
    destination.mkdir(parents=True, exist_ok=False)
    try:
        contents = destination / "Contents"
        (contents / "MacOS").mkdir(parents=True)
        (contents / "Resources").mkdir()
        shutil.copyfile(binary, contents / "MacOS/junto")
        (contents / "MacOS/junto").chmod(0o755)
        if recovery is not None:
            shutil.copyfile(recovery, contents / "MacOS/junto-recover")
            (contents / "MacOS/junto-recover").chmod(0o755)
        info = {
            "CFBundleIdentifier": "org.junto.app",
            "CFBundleName": "Junto",
            "CFBundleDisplayName": "Junto",
            "CFBundleExecutable": "junto",
            "CFBundlePackageType": "APPL",
            "CFBundleShortVersionString": version,
            "CFBundleVersion": version,
            "CFBundleIconFile": "Junto.icns",
            "NSHighResolutionCapable": True,
            "NSMicrophoneUsageDescription": "Junto uses the microphone when you join a voice call.",
            "NSCameraUsageDescription": "Junto uses the camera when you turn on video in a call.",
            "NSScreenCaptureUsageDescription": "Junto shares the screen or window you choose in a call.",
        }
        (contents / "Info.plist").write_bytes(plistlib.dumps(info))
        icons.run(icon_tool, "icns", contents / "Resources/Junto.icns")
        return destination
    except BaseException:
        shutil.rmtree(destination)
        raise


if __name__ == "__main__":
    if len(sys.argv) != 5:
        sys.exit("usage: packaging/bundle.py <release binary> <new Junto.app path> <recovery program> <icon program>")
    root = pathlib.Path(__file__).resolve().parent.parent
    version = tomllib.loads((root / "Cargo.toml").read_text())["workspace"]["package"]["version"]
    try:
        assemble(sys.argv[1], sys.argv[2], os.environ.get("JUNTO_TEST_BUILD_VERSION", version), sys.argv[3],
                 icon_tool=sys.argv[4])
    except ValueError as error:
        sys.exit(str(error))
