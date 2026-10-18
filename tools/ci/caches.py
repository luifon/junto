#!/usr/bin/env python3
"""Keeps the caches of a CI runner within their limits (ADR 0905):

    caches.py <directory> <runner> before|after <run>

<directory> is the CI directory of the machine, <runner> "linux", "macos" or "windows", <run>
the name of the run. tools/ci/run-jobs.sh runs it with the run lock held, before the first job
of a run and after the processes of its last job have ended. On the Linux host the environment
gives JUNTO_CI_PYTHON_IMAGE, the image of the containers that measure the volumes.

The caches: on the Linux host the Docker volumes junto-ci-target (Cargo target directory),
junto-ci-cargo (Cargo registry), junto-ci-uv and junto-ci-npm; on macOS the Cargo target
directory <directory>/cargo-target and the BuildKit builder junto-ci-deployment of the
deployment job; on Windows <directory>/cargo-target and the Cargo registry
<directory>/cargo/registry.

Before a run:

1. The target cache is emptied when it holds output that this runner did not write: after each
   run a stamp in it (.junto-ci-runner) names the version of the runner and the path of the
   checkout, and the target is emptied when the stamp is absent, names another version or path,
   or when a fingerprint file of Cargo is newer than the stamp. A run of another runner, such as
   the run-jobs.sh of an older commit with a checkout of its own, wrote such output: Cargo would
   keep it, compiled from other files and naming another checkout.
2. A cache above its size cap is emptied: Cargo and BuildKit keep the output of earlier builds.
3. The free space that the run will use is estimated from the drops of the free space during
   the last RECORDS runs of the machine that started in the same state (with the target cache,
   or without it), recorded in <directory>/cache-history.tsv: the median of those drops, with
   the default added until there are MINIMUM values, so that one run during which other work
   used much of the disk does not decide. The default of a run with the target cache is
   JUNTO_CI_RUN_GIB; that of a run without it is JUNTO_CI_COLD_RUN_GIB, and when the caches
   would be emptied for the run, at least their present size and the use of a run with them.
4. When the free space less that use is at least JUNTO_CI_FREE_GIB, the caches stay. Otherwise,
   when a run without caches would end with that much free space, every cache is emptied. Else
   the run starts no job (exit status 1).

After a run only the caps apply, and the stamp is written. The free space decides only before a
run: a cache that the next run fills again releases no space for that run.

The limits are in GiB. Each is read from a line NAME=VALUE of <directory>/settings, and has its
default when the file has no such line: JUNTO_CI_TARGET_CAP_GIB=60, JUNTO_CI_CARGO_CAP_GIB=8,
JUNTO_CI_UV_CAP_GIB=4, JUNTO_CI_NPM_CAP_GIB=4, JUNTO_CI_BUILDER_CAP_GIB=40, JUNTO_CI_FREE_GIB=20
(the free space that must remain at the end of a run), JUNTO_CI_RUN_GIB=10 (the use of a run
with the target cache while the machine has no record of one: such runs used 0.7 GiB on the
Linux host and grew the target directory of the Windows machine by 4.1 GB),
JUNTO_CI_COLD_RUN_GIB=40 (the use of a run without the target cache while the machine has no
record of one: such a run lowered the free space of the macOS machine by 29.3 GiB).

Each measurement and removal is written to the standard output. Exit status 1: the run starts no
job for lack of free space. Exit status 2: a setting, a size or the free space could not be read;
before a run the run then starts no job, and no cache is removed.
"""
import os
from pathlib import Path
import re
import shutil
import stat
import statistics
import subprocess
import sys
import time

GIB = 1024 * 1024  # KiB
RECORDS = 5
MINIMUM = 3
# The version of the way the runner writes its checkout and builds: 2 is one checkout per
# machine (tools/ci/run-jobs.sh). A change of it empties the target caches once.
RUNNER = 2
STAMP = ".junto-ci-runner"
BUILDER = "junto-ci-deployment"
LIMITS = {
    "JUNTO_CI_TARGET_CAP_GIB": 60,
    "JUNTO_CI_CARGO_CAP_GIB": 8,
    "JUNTO_CI_UV_CAP_GIB": 4,
    "JUNTO_CI_NPM_CAP_GIB": 4,
    "JUNTO_CI_BUILDER_CAP_GIB": 40,
    "JUNTO_CI_FREE_GIB": 20,
    "JUNTO_CI_RUN_GIB": 10,
    "JUNTO_CI_COLD_RUN_GIB": 40,
}


class Failed(Exception):
    """A setting, a size or the free space that cannot be read."""


def settings(directory):
    """The limits in KiB, from <directory>/settings and the defaults."""
    limits = {name: value * GIB for name, value in LIMITS.items()}
    path = Path(directory) / "settings"
    if not path.exists():
        return limits
    for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        line = line.strip()  # also the CR of a file with CR LF line ends
        if not line or line.startswith("#"):
            continue
        name, separator, value = line.partition("=")
        name, value = name.strip(), value.strip()
        if not separator or name not in LIMITS:
            raise Failed(f"{path}:{number}: not a setting of caches.py: {line}")
        if not value.isascii() or not value.isdigit():
            raise Failed(f"{path}:{number}: {name} is not a whole number of GiB: {value}")
        limits[name] = int(value, 10) * GIB
    return limits


def directory_size(top):
    """The size in KiB of the files below a directory: the sum of their sizes, read from the
    listing of each directory. On Windows du of Git for Windows opens each file, which took
    11.5 minutes for 35 GB of new build output; the listing took 1.4 s. A file with several
    names is counted once where the listing states its identity (not on Windows, where
    os.scandir reads FindFirstFileW, which gives no file ID). Links and junctions are not
    followed."""
    if os.name == "nt":
        top = "\\\\?\\" + os.path.abspath(top)  # paths longer than 260 characters
    seen = set()
    total = 0
    directories = [top]
    while directories:
        with os.scandir(directories.pop()) as entries:
            for entry in entries:
                if entry.is_symlink() or getattr(entry, "is_junction", lambda: False)():
                    continue
                if entry.is_dir(follow_symlinks=False):
                    directories.append(entry.path)
                elif entry.is_file(follow_symlinks=False):
                    status = entry.stat(follow_symlinks=False)
                    if status.st_nlink > 1 and status.st_ino:
                        if (status.st_dev, status.st_ino) in seen:
                            continue
                        seen.add((status.st_dev, status.st_ino))
                    total += status.st_size
    return total // 1024


def whole(text, what):
    text = text.strip()
    if not text.isascii() or not text.isdigit():
        raise Failed(f"{what} is not a number: {text!r}")
    return int(text, 10)


def program(arguments, what):
    try:
        done = subprocess.run(arguments, capture_output=True, text=True, timeout=1800)
    except (OSError, subprocess.TimeoutExpired) as error:
        raise Failed(f"{what}: {error}") from error
    if done.returncode != 0:
        raise Failed(f"{what}: exit status {done.returncode}: {done.stderr.strip()[-300:]}")
    return done.stdout


class Linux:
    """The Docker volumes of the Linux host, read and emptied in containers."""

    def __init__(self, limits):
        self.image = os.environ.get("JUNTO_CI_PYTHON_IMAGE", "")
        if not self.image:
            raise Failed("JUNTO_CI_PYTHON_IMAGE is not set")
        self.caches = [("junto-ci-target", limits["JUNTO_CI_TARGET_CAP_GIB"]),
                       ("junto-ci-cargo", limits["JUNTO_CI_CARGO_CAP_GIB"]),
                       ("junto-ci-uv", limits["JUNTO_CI_UV_CAP_GIB"]),
                       ("junto-ci-npm", limits["JUNTO_CI_NPM_CAP_GIB"])]
        self.target = "junto-ci-target"

    def container(self, purpose, volume, command):
        return program(["docker", "run", "--rm", "--name", f"junto-ci-cache-{purpose}", "--network", "none",
                        "--cpus", "1", "--memory", "256m", "--memory-swap", "256m", "-v", f"{volume}:/cache",
                        self.image, *command], f"the {purpose} of {volume}")

    def size(self, name):
        if subprocess.run(["docker", "volume", "inspect", name], capture_output=True).returncode != 0:
            return None
        return whole(self.container("size", name, ["du", "-sk", "/cache"]).split("\t")[0], f"the size of {name}")

    def free(self):
        lines = self.container("free", self.target, ["df", "-Pk", "/cache"]).splitlines()
        return whole(lines[-1].split()[3] if len(lines) > 1 else "", "the free space of the Docker volumes")

    def empty(self, name):
        self.container("empty", name, ["find", "/cache", "-mindepth", "1", "-delete"])

    def foreign(self, stamp):
        """Why the target volume holds output of another runner, or None."""
        output = self.container("stamp", self.target, ["sh", "-c", f"cat /cache/{STAMP} 2>/dev/null || true; echo '~'; "
                                f"if [ -f /cache/{STAMP} ]; then find /cache -path '*/.fingerprint/*' "
                                f"-newer /cache/{STAMP} -print -quit; fi"])
        written, _, newer = output.partition("~\n")
        return foreign_reason(written, stamp, bool(newer.strip()))

    def mark(self, stamp):
        self.container("stamp", self.target, ["sh", "-c", f"printf '%s\\n' \"$1\" >/cache/{STAMP}", "sh", stamp])


class Local:
    """The caches of the macOS machine and of the Windows machine: directories of the CI
    directory, and on macOS the BuildKit builder of the deployment job."""

    def __init__(self, directory, runner, limits):
        self.directory = Path(directory)
        self.runner = runner
        self.caches = [("cargo-target", limits["JUNTO_CI_TARGET_CAP_GIB"])]
        if runner == "windows":
            self.caches.append(("cargo/registry", limits["JUNTO_CI_CARGO_CAP_GIB"]))
        else:
            self.caches.append((f"builder {BUILDER}", limits["JUNTO_CI_BUILDER_CAP_GIB"]))
        self.target = "cargo-target"

    def size(self, name):
        if name.startswith("builder "):
            names = program(["docker", "buildx", "ls", "--format", "{{.Name}}"], "the list of BuildKit builders")
            if BUILDER not in names.split():
                return None
            output = program(["docker", "buildx", "du", "--builder", BUILDER], f"the size of the builder {BUILDER}")
            found = re.search(r"^Total:\s+([0-9.]+)\s*([kMGT]?B)\s*$", output, re.MULTILINE)
            if not found:
                raise Failed(f"the size of the builder {BUILDER} is not in: {output.strip()[-200:]!r}")
            factor = {"B": 1, "kB": 1e3, "MB": 1e6, "GB": 1e9, "TB": 1e12}[found.group(2)]
            return int(float(found.group(1)) * factor) // 1024
        path = self.directory / name
        if not path.is_dir():
            return None
        try:
            return directory_size(path)
        except OSError as error:
            raise Failed(f"the size of {path}: {error}") from error

    def free(self):
        try:
            return shutil.disk_usage(self.directory).free // 1024
        except OSError as error:
            raise Failed(f"the free space of {self.directory}: {error}") from error

    def empty(self, name):
        if name.startswith("builder "):
            program(["docker", "buildx", "prune", "--builder", BUILDER, "--all", "--force"],
                    f"the removal of the cache of the builder {BUILDER}")
            return
        path = str(self.directory / name)
        if os.name == "nt":
            path = "\\\\?\\" + os.path.abspath(path)

        def writable(function, failed, _):
            os.chmod(failed, stat.S_IWRITE)
            function(failed)

        if sys.version_info >= (3, 12):
            shutil.rmtree(path, onexc=writable)
        else:
            shutil.rmtree(path, onerror=writable)

    def foreign(self, stamp):
        """Why the target directory holds output of another runner, or None."""
        target = self.directory / self.target
        try:
            written = (target / STAMP).read_text(encoding="utf-8", errors="replace")
            since = (target / STAMP).stat().st_mtime_ns
        except FileNotFoundError:
            return foreign_reason("", stamp, False)
        except OSError as error:
            raise Failed(f"the stamp of {target}: {error}") from error
        try:
            for fingerprints in [*target.glob("*/.fingerprint"), *target.glob("*/*/.fingerprint")]:
                for unit in os.scandir(fingerprints):
                    if unit.is_dir(follow_symlinks=False):
                        for entry in os.scandir(unit.path):
                            if entry.stat(follow_symlinks=False).st_mtime_ns > since:
                                return foreign_reason(written, stamp, True)
        except OSError as error:
            raise Failed(f"the fingerprints of {target}: {error}") from error
        return foreign_reason(written, stamp, False)

    def mark(self, stamp):
        (self.directory / self.target / STAMP).write_text(stamp + "\n", encoding="utf-8")


def history(directory):
    """The drop of the free space during each earlier run with both records, oldest first:
    (whether the run started with the target cache, the drop in KiB)."""
    path = Path(directory) / "cache-history.tsv"
    if not path.exists():
        return []
    started = {}
    drops = []
    for line in lines_of(path):
        fields = line.split("\t")
        if len(fields) != 4 or not fields[2].isdigit() or not fields[3].isdigit():
            continue
        run, phase, free, target = fields[0], fields[1], int(fields[2]), int(fields[3])
        if phase == "before":
            started[run] = (target > 0, free)
        elif phase == "after" and run in started:
            warm, before = started.pop(run)
            drops.append((warm, max(0, before - free)))
    return drops


def lines_of(path):
    """The lines of the history with four fields; a line that a crash cut or that is not text is
    left out."""
    if not path.exists():
        return []
    try:
        text = path.read_bytes().decode("utf-8", errors="replace")
    except OSError as error:
        raise Failed(f"{path}: {error}") from error
    return [line for line in text.splitlines()
            if len(line.split("\t")) == 4 and all(field.isdigit() for field in line.split("\t")[2:])]


def record(directory, run, phase, free, target):
    path = Path(directory) / "cache-history.tsv"
    lines = lines_of(path)
    lines.append(f"{run}\t{phase}\t{free}\t{target or 0}")
    # Two lines per run; enough runs of each state remain to be read.
    path.write_text("".join(f"{line}\n" for line in lines[-100:]), encoding="utf-8")


def estimate(drops, default):
    """The median of the last RECORDS drops, with the default added until there are MINIMUM."""
    values = drops[-RECORDS:]
    values = values + [default] * max(0, MINIMUM - len(values))
    return int(statistics.median(values))


def foreign_reason(written, stamp, newer):
    if not written:
        return "it has no stamp of this runner"
    if written.strip() != stamp:
        return f"its stamp is {written.strip()!r}, not {stamp!r}"
    if newer:
        return "a fingerprint of Cargo is newer than the stamp of the last run of this runner"
    return None


def plan(free, sizes, limits, drops):
    """The decision before a run, from the free space and the sizes in KiB that remain after the
    caps, the target cache first: ("keep" | "empty" | "refuse", the use of a run in KiB)."""
    level = limits["JUNTO_CI_FREE_GIB"]
    warm_drops = [drop for warm, drop in drops if warm]
    cold_drops = [drop for warm, drop in drops if not warm]
    warm_use = estimate(warm_drops, limits["JUNTO_CI_RUN_GIB"])
    total = sum(sizes)
    if not sizes or not sizes[0]:
        cold_use = estimate(cold_drops, limits["JUNTO_CI_COLD_RUN_GIB"])
        return ("keep" if free - cold_use >= level else "refuse"), cold_use
    if free - warm_use >= level:
        return "keep", warm_use
    cold_use = estimate(cold_drops, max(limits["JUNTO_CI_COLD_RUN_GIB"], total + warm_use))
    if free + total - cold_use >= level:
        return "empty", cold_use
    return "refuse", warm_use


def log(phase, message):
    print(f"{time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime())}  {phase}: {message}", flush=True)


def check(directory, runner, phase, run):
    limits = settings(directory)
    machine = Linux(limits) if runner == "linux" else Local(directory, runner, limits)
    # The path at which the jobs build: the containers of the Linux host mount the checkout at /src.
    stamp = f"runner {RUNNER} checkout {'/src' if runner == 'linux' else Path(directory) / 'checkout'}"
    remaining = []
    for name, cap in machine.caches:
        size = machine.size(name)
        if size is None:
            continue
        if phase == "before" and name == machine.target:
            reason = machine.foreign(stamp)
            if reason:
                machine.empty(name)
                log(phase, f"cache {name}: {size} KiB; emptied, because {reason}")
                size = 0
                remaining.append((name, size))
                continue
        if size > cap:
            machine.empty(name)
            log(phase, f"cache {name}: {size} KiB, above its cap of {cap} KiB; emptied")
            size = 0
        else:
            log(phase, f"cache {name}: {size} KiB of {cap} KiB")
        remaining.append((name, size))
    free = machine.free()
    target = dict(remaining).get(machine.target, 0)
    if phase == "after":
        if target:
            machine.mark(stamp)
        log(phase, f"free space: {free} KiB")
        record(directory, run, phase, free, target)
        return 0
    level = limits["JUNTO_CI_FREE_GIB"]
    sizes = [target] + [size for name, size in remaining if name != machine.target]
    decision, use = plan(free, sizes, limits, history(directory))
    log(phase, f"free space: {free} KiB; a run uses up to {use} KiB; at least {level} KiB must remain")
    if decision == "empty":
        for name, size in sorted(remaining, key=lambda cache: -cache[1]):
            if size:
                machine.empty(name)
                log(phase, f"cache {name} emptied: a run without caches leaves enough free space")
        free, target = machine.free(), 0
        log(phase, f"free space: {free} KiB")
    elif decision == "refuse":
        print(f"the run starts no job: the disk of the caches has {free} KiB free, a run uses up to {use} KiB, "
              f"and {level} KiB must remain (JUNTO_CI_FREE_GIB and JUNTO_CI_RUN_GIB in {Path(directory) / 'settings'})",
              file=sys.stderr)
        return 1
    record(directory, run, phase, free, target)
    return 0


def main(arguments):
    if len(arguments) != 4 or arguments[1] not in ("linux", "macos", "windows") \
            or arguments[2] not in ("before", "after"):
        print("usage: caches.py <directory> <linux|macos|windows> <before|after> <run>", file=sys.stderr)
        return 2
    try:
        return check(*arguments)
    except Failed as error:
        print(f"caches.py: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
