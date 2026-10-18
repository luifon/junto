#!/usr/bin/env python3
"""Prints the size in KiB of the files below a directory: the sum of their sizes, as
tools/ci/caches.sh reads a cache on macOS and on Windows (ADR 0905).

The sizes come from the listing of each directory. On Windows, du of Git for Windows opens each
file to read its size, which took 11 minutes for 35 GB of new build output; the listing takes
seconds. A file with several names is counted once where the system states its identity in the
listing (not on Windows). Links are not followed."""
import os
import sys


def size(top):
    if os.name == "nt":
        # Paths in a Cargo target directory can be longer than 260 characters.
        top = "\\\\?\\" + os.path.abspath(top)
    seen = set()
    total = 0
    directories = [top]
    while directories:
        with os.scandir(directories.pop()) as entries:
            for entry in entries:
                if entry.is_dir(follow_symlinks=False):
                    directories.append(entry.path)
                elif entry.is_file(follow_symlinks=False):
                    status = entry.stat(follow_symlinks=False)
                    if status.st_nlink > 1 and status.st_ino:
                        if (status.st_dev, status.st_ino) in seen:
                            continue
                        seen.add((status.st_dev, status.st_ino))
                    total += status.st_size
    return total


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit(f"usage: {sys.argv[0]} <directory>")
    print(size(sys.argv[1]) // 1024)
