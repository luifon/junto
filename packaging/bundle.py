#!/usr/bin/env python3
"""Assemble a macOS application from the release binary; no application is launched."""
import os
import pathlib
import plistlib
import re
import shutil
import struct
import sys
import tomllib
import zlib


def icon():
    # The application mark is two joined rings in the interface's cypress colour.
    size = 128
    pixels = bytearray()
    for y in range(size):
        pixels.append(0)
        for x in range(size):
            ring = any(18**2 <= (x - center)**2 + (y - 64)**2 <= 28**2 for center in (47, 81))
            pixels.extend((40, 83, 65, 255) if ring else (244, 240, 231, 255))
    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))
    png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(pixels)) + chunk(b"IEND", b"")
    return b"icns" + struct.pack(">I", 16 + len(png)) + b"ic07" + struct.pack(">I", 8 + len(png)) + png


def assemble(binary, destination, version, recovery=None):
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
            shutil.copyfile(recovery, contents / "MacOS/junto-release")
            (contents / "MacOS/junto-release").chmod(0o755)
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
        (contents / "Resources/Junto.icns").write_bytes(icon())
        return destination
    except BaseException:
        shutil.rmtree(destination)
        raise


if __name__ == "__main__":
    if len(sys.argv) != 4:
        sys.exit("usage: packaging/bundle.py <release binary> <new Junto.app path> <recovery binary>")
    root = pathlib.Path(__file__).resolve().parent.parent
    version = tomllib.loads((root / "Cargo.toml").read_text())["workspace"]["package"]["version"]
    assemble(sys.argv[1], sys.argv[2], os.environ.get("JUNTO_TEST_BUILD_VERSION", version), sys.argv[3])
