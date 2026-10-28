#!/usr/bin/env python3
"""The disk image of a macOS release (ADR 0728): a read-only compressed image with the volume
name Junto that holds the signed `Junto.app` and a link to the Applications folder, so that a
person installs Junto by dragging it onto the link. The release signs the image with the
identity of the bundle; the update path stays the signed archive (ADR 0726)."""
import pathlib
import subprocess
import sys
import tempfile

VOLUME = "Junto"


def run(*arguments):
    subprocess.run([str(argument) for argument in arguments], check=True, timeout=600,
                   stdout=subprocess.DEVNULL)


def assemble(bundle, output, *, command=run):
    """Writes the image `output` from `bundle`; `output` must not exist."""
    bundle, output = pathlib.Path(bundle), pathlib.Path(output)
    if bundle.name != "Junto.app" or not (bundle / "Contents/Info.plist").is_file():
        raise ValueError(f"{bundle} is not Junto.app")
    if output.suffix != ".dmg":
        raise ValueError("the image name ends with .dmg")
    if output.exists() or output.is_symlink():
        raise FileExistsError(output)
    with tempfile.TemporaryDirectory(dir=output.parent, prefix=".dmg-") as staging:
        volume = pathlib.Path(staging) / VOLUME
        volume.mkdir()
        # ditto keeps the signature, the extended attributes and the modes of the bundle.
        command("/usr/bin/ditto", bundle, volume / "Junto.app")
        (volume / "Applications").symlink_to("/Applications")
        command("/usr/bin/hdiutil", "create", "-volname", VOLUME, "-srcfolder", volume, "-fs", "HFS+",
                "-format", "UDZO", "-imagekey", "zlib-level=9", output)
    if not output.is_file():
        raise RuntimeError("hdiutil wrote no image")
    return output


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit("usage: packaging/dmg.py <Junto.app> <new image.dmg>")
    try:
        assemble(sys.argv[1], sys.argv[2])
    except (ValueError, OSError, RuntimeError, subprocess.CalledProcessError) as error:
        sys.exit(str(error))
