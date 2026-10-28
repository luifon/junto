#!/usr/bin/env python3
"""Assemble a macOS application from the release binary; no application is launched."""
import os
import pathlib
import plistlib
import re
import shutil
import subprocess
import sys
import tempfile
import tomllib

import icons
import licences
from configuration import architecture


# The Icon Composer document of the icon of macOS 26 (ADR 0756): the discs of the source as one
# layer on the fill of its background, to which the system adds its own light and depth.
ICON_DOCUMENT = pathlib.Path(__file__).resolve().parent / "icon/Junto.icon"


def compile_icon(document, resources):
    """Compiles the Icon Composer document with actool of Xcode into `resources/Assets.car`."""
    with tempfile.TemporaryDirectory() as output:
        subprocess.run(["/usr/bin/xcrun", "actool", str(document), "--compile", output, "--app-icon", "Junto",
                        "--include-all-app-icons", "--enable-on-demand-resources", "NO",
                        "--development-region", "en", "--target-device", "mac", "--platform", "macosx",
                        "--minimum-deployment-target", "13.0",
                        "--output-partial-info-plist", str(pathlib.Path(output) / "partial.plist"),
                        "--output-format", "human-readable-text", "--errors", "--warnings"],
                       check=True, capture_output=True, timeout=300)
        shutil.copyfile(pathlib.Path(output) / "Assets.car", pathlib.Path(resources) / "Assets.car")


def assemble(binary, destination, version, recovery=None, *, icon_tool, build, icon_compiler=None):
    """Writes `destination`; `icon_tool` is the program `junto-icon` that writes the icon file, and
    `build` the directory of the programs of the build, whose licences the bundle carries. On macOS
    the bundle also holds the compiled Icon Composer icon, which `icon_compiler` writes (actool of
    Xcode unless a test gives another); elsewhere it holds the icon file alone."""
    if icon_compiler is None and sys.platform == "darwin":
        icon_compiler = compile_icon
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
            "NSDownloadsFolderUsageDescription": "Junto saves the files of messages and recordings into Downloads.",
            "NSLocalNetworkUsageDescription": "Junto connects to servers on your local network.",
        }
        if icon_compiler is not None:
            icon_compiler(ICON_DOCUMENT, contents / "Resources")
            # macOS 26 draws the icon of the asset catalog; earlier versions read Junto.icns.
            info["CFBundleIconName"] = "Junto"
        (contents / "Info.plist").write_bytes(plistlib.dumps(info))
        icons.run(icon_tool, "icns", contents / "Resources/Junto.icns")
        (contents / "Resources/licences").mkdir()
        for name, text in licences.package_files(version, licences.target("macos", architecture()), build).items():
            (contents / "Resources/licences" / name).write_bytes(text)
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
                 icon_tool=sys.argv[4], build=pathlib.Path(sys.argv[1]).parent)
    except ValueError as error:
        sys.exit(str(error))
