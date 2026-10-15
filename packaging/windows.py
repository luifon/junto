#!/usr/bin/env python3
"""Assemble the Windows package of a client build: a ZIP file with the folder `Junto`, and the
source directory of its update archive (ADR 0755). The program carries the static C runtime
(ADR 0753) and needs no other library of its own."""
import argparse
import json
import os
import pathlib
import re
import shutil
import sys
import time
import zipfile

ROOT = pathlib.Path(__file__).resolve().parent.parent


def description(version, architecture):
    return json.dumps({"format": 1, "version": version, "system": "windows", "package": "directory",
                       "architecture": architecture}) + "\n"


def licence_texts():
    texts = {"Junto-LICENSE.txt": (ROOT / "LICENSE").read_bytes()}
    for path in sorted((ROOT / "crates/client/assets/fonts").glob("*-OFL.txt")):
        texts[path.name] = path.read_bytes()
    return texts


def assemble(client, recover, version, output, architecture="x86_64", epoch=None):
    """Writes `output/package-directory/Junto` and `output/Junto-<version>-windows-<architecture>.zip`."""
    client, recover, output = pathlib.Path(client), pathlib.Path(recover), pathlib.Path(output)
    if not re.fullmatch(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)", version):
        raise ValueError("a release version has three decimal numbers")
    for program in [client, recover]:
        if not program.is_file() or program.read_bytes()[:2] != b"MZ":
            raise ValueError(f"{program} is not a Windows program")
    epoch = int(os.environ.get("SOURCE_DATE_EPOCH", "315532800")) if epoch is None else epoch
    output.mkdir(parents=False, exist_ok=False)
    try:
        folder = output / "package-directory/Junto"
        (folder / "licences").mkdir(parents=True)
        shutil.copyfile(client, folder / "junto.exe")
        shutil.copyfile(recover, folder / "junto-recover.exe")
        (folder / "junto-package.json").write_text(description(version, architecture), newline="\n")
        for name, text in licence_texts().items():
            (folder / "licences" / name).write_bytes(text)
        moment = time.gmtime(max(epoch, 315532800))[:6]
        with zipfile.ZipFile(output / f"Junto-{version}-windows-{architecture}.zip", "w",
                             compression=zipfile.ZIP_DEFLATED) as archive:
            for path in [folder, *sorted(folder.rglob("*"))]:
                name = path.relative_to(folder.parent).as_posix() + ("/" if path.is_dir() else "")
                entry = zipfile.ZipInfo(name, date_time=moment)
                entry.compress_type = zipfile.ZIP_DEFLATED
                entry.external_attr = (0o40755 << 16) | 0x10 if path.is_dir() else 0o100644 << 16
                archive.writestr(entry, b"" if path.is_dir() else path.read_bytes())
    except BaseException:
        shutil.rmtree(output)
        raise
    return output


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Assemble the Windows package of a client build.")
    parser.add_argument("client", help="junto.exe")
    parser.add_argument("recover", help="junto-recover.exe")
    parser.add_argument("version")
    parser.add_argument("output", help="a new directory")
    arguments = parser.parse_args()
    try:
        assemble(arguments.client, arguments.recover, arguments.version, arguments.output)
    except (ValueError, OSError) as error:
        sys.exit(str(error))
