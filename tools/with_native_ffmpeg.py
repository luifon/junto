"""Runs a command with JUNTO_FFMPEG and JUNTO_FFPROBE naming the pinned FFmpeg build for Windows
(ADR 0905). The tests of recordings need `ffmpeg` and `ffprobe` with the encoders `libopus` and
`libx264` (ADR 0606); the Windows machine has no FFmpeg of its own. The folder of the two
programs also comes first on PATH.

    python3 tools/with_native_ffmpeg.py <command> [arguments]
    python3 tools/with_native_ffmpeg.py --fetch

The build is the "essentials" build of the FFmpeg version of the server image
(deploy/images/server/Dockerfile), which has both encoders. Its ZIP is downloaded once into
JUNTO_TEST_FFMPEG_DOWNLOADS (default: .cache/junto-ffmpeg in the home folder) and checked by
its SHA-256; the two programs are extracted beside it once for each download. `--fetch` only
downloads, checks and extracts.
"""

import os
import pathlib
import subprocess
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import native_downloads  # noqa: E402

# The version of FFMPEG_VERSION in deploy/images/server/Dockerfile; tools/check-pins.sh compares
# them.
VERSION = "9.0.2"
NAME = f"ffmpeg-{VERSION}-essentials_build"
URL = f"https://github.com/GyanD/codexffmpeg/releases/download/{VERSION}/{NAME}.zip"
SHA256 = "60f467265b1e312373dbcd92200c2618a74850f98d3d078e94296bb3fa2047ba"
PROGRAMS = [f"{NAME}/bin/ffmpeg.exe", f"{NAME}/bin/ffprobe.exe"]


def programs(downloads, fetch=native_downloads.fetch, extract=native_downloads.extract):
    """The paths of ffmpeg and ffprobe of the pinned build, extracted from a checked download."""
    archive = fetch(f"FFmpeg {VERSION}", URL, SHA256, downloads)
    folder = downloads / NAME
    stamp = folder / "extracted-from"
    if stamp.exists() and stamp.read_text() == SHA256:
        return [folder / pathlib.PurePosixPath(name).name for name in PROGRAMS]
    extracted = extract(archive, PROGRAMS, folder)
    stamp.write_text(SHA256)
    return extracted


def main(arguments):
    if not arguments:
        print("usage: with_native_ffmpeg.py --fetch | <command> [arguments]", file=sys.stderr)
        return 2
    if sys.platform != "win32":
        print("the pinned FFmpeg build is one for Windows", file=sys.stderr)
        return 2
    default = pathlib.Path.home() / ".cache" / "junto-ffmpeg"
    downloads = pathlib.Path(os.environ.get("JUNTO_TEST_FFMPEG_DOWNLOADS", default))
    try:
        ffmpeg, ffprobe = programs(downloads)
    except (native_downloads.Failure, OSError) as failure:
        print(f"FFmpeg {VERSION} is not available: {failure}", flush=True)
        return 1
    if arguments == ["--fetch"]:
        return 0
    # The unit tests of the tools of recordings run the programs by their names.
    path = os.pathsep.join([str(ffmpeg.parent), os.environ.get("PATH", "")])
    environment = {**os.environ, "JUNTO_FFMPEG": str(ffmpeg), "JUNTO_FFPROBE": str(ffprobe), "PATH": path}
    return subprocess.run(arguments, env=environment).returncode


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
