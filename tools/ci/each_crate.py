#!/usr/bin/env python3
"""Runs one Cargo command for each package of the workspace alone (ADR 0905):

    each_crate.py <subcommand> [arguments]

runs `cargo <subcommand> -p <package> [arguments]` for every package that `cargo metadata
--no-deps` lists, in its order. A package alone gets only the features that it and its own
dependencies ask for, so a crate that compiles only with a feature that another package of the
workspace turns on fails here and not in the lint of the whole workspace. Every package runs
also after a failed one; each prints its duration, and the program ends with the list of the
failed packages and the exit status of the first.
"""
import json
import os
import subprocess
import sys
import time


def packages(cargo):
    output = subprocess.run([cargo, "metadata", "--no-deps", "--format-version", "1", "--locked"],
                            capture_output=True, text=True, check=True).stdout
    return [package["name"] for package in json.loads(output)["packages"]]


def run(arguments, cargo=None, out=sys.stdout):
    cargo = cargo or os.environ.get("CARGO", "cargo")
    subcommand, rest = arguments[0], arguments[1:]
    first, failed = 0, []
    for package in packages(cargo):
        command = [cargo, subcommand, "-p", package, *rest]
        print(f"=== {package}: {' '.join(command)}", file=out, flush=True)
        started = time.monotonic()
        status = subprocess.run(command).returncode
        seconds = round(time.monotonic() - started)
        print(f"=== {package}: exit {status} after {seconds // 60}m{seconds % 60:02d}s", file=out, flush=True)
        if status != 0:
            first = first or status
            failed.append(f"{package} (exit {status})")
    if failed:
        print(f"=== failed packages: {', '.join(failed)}", file=out, flush=True)
    return first


if __name__ == "__main__":
    if len(sys.argv) < 2:
        print("usage: each_crate.py <subcommand> [arguments]", file=sys.stderr)
        sys.exit(2)
    try:
        sys.exit(run(sys.argv[1:]))
    except (subprocess.CalledProcessError, OSError, ValueError, KeyError) as error:
        print(f"the packages of the workspace were not read: {error}", file=sys.stderr)
        sys.exit(2)
