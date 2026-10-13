#!/usr/bin/env python3
"""Requires --data-dir, --no-activate and --check-startup from the later startup-support merge.
Check the bundle's first frame using the client's explicit startup-check mode."""
import pathlib
import subprocess
import sys
import tempfile


def check(bundle):
    binary = pathlib.Path(bundle).resolve(strict=True) / "Contents/MacOS/junto"
    with tempfile.TemporaryDirectory(prefix="junto-start-") as directory:
        result = subprocess.run([str(binary), "--data-dir", directory, "--no-activate", "--check-startup"],
                                capture_output=True, text=True, timeout=120)
        if result.returncode or "Junto first frame rendered" not in (result.stdout + result.stderr).splitlines():
            sys.exit("the bundle did not confirm its first frame")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit("usage: packaging/check_start.py <Junto.app>")
    check(sys.argv[1])
