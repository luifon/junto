#!/usr/bin/env python3
"""Print the key of the inputs of the Linux build image (ADR 0750, ADR 0905):

    image_inputs.py <context directory> <build argument>...

The key is a SHA-256 over the path, the executable bit and the content of every file below the
context directory, which holds the Dockerfile, and over each build argument NAME=VALUE. The
Dockerfile names its base image by digest and its packages by version from a dated snapshot, so
two builds with one key give the same image. tools/linux/run.sh labels the image with the key and
builds only when the label of the existing image differs.
"""
import hashlib
import os
from pathlib import Path
import sys


def key(context, arguments):
    digest = hashlib.sha256()
    root = Path(context)
    if not (root / "Dockerfile").is_file():
        raise ValueError(f"{context} has no Dockerfile")
    files = sorted(path for path in root.rglob("*") if path.is_file() and "__pycache__" not in path.parts)
    for path in files:
        name = path.relative_to(root).as_posix().encode()
        executable = b"x" if os.access(path, os.X_OK) else b"-"
        content = path.read_bytes()
        digest.update(b"file\0" + name + b"\0" + executable + b"\0" + str(len(content)).encode() + b"\0")
        digest.update(content)
    for argument in arguments:
        if "=" not in argument:
            raise ValueError(f"a build argument is NAME=VALUE: {argument}")
        digest.update(b"argument\0" + argument.encode() + b"\0")
    return digest.hexdigest()[:32]


if __name__ == "__main__":
    if len(sys.argv) < 2:
        print("usage: image_inputs.py <context directory> [NAME=VALUE]...", file=sys.stderr)
        sys.exit(2)
    try:
        print(key(sys.argv[1], sys.argv[2:]))
    except (OSError, ValueError) as error:
        print(error, file=sys.stderr)
        sys.exit(2)
