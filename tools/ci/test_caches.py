#!/usr/bin/env python3
"""Tests of tools/ci/caches.py and of its use by tools/ci/run-jobs.sh. A cache of the macOS and
Windows runners is a directory with a sparse file named data, of the size of the cache. A
program named docker in the PATH stands for the Docker daemon: its volumes are directories
below FAKE_ROOT/volumes, the free space of its disk is FAKE_DISK KiB less the sizes of the files
named data below FAKE_ROOT, and FAKE_BUILDER_KIB is the size of the BuildKit builder of the
deployment job."""
import importlib.util
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

CI = Path(__file__).resolve().parent
GIB = 1024 * 1024
specification = importlib.util.spec_from_file_location("caches", CI / "caches.py")
caches = importlib.util.module_from_spec(specification)
specification.loader.exec_module(caches)

# docker volume inspect <name>; docker run ... -v <name>:/cache <image> du|df|find ...;
# docker buildx ls|du|prune; docker info; the other commands of the sweep of run-jobs.sh list
# nothing. Each call is recorded in docker.log, with "locked" when the file LOCK_MARKER exists.
DOCKER = r'''#!/usr/bin/env python3
import os
from pathlib import Path
import sys
root = Path(os.environ["FAKE_ROOT"])
arguments = sys.argv[1:]
marker = os.environ.get("LOCK_MARKER")
with open(root / "docker.log", "a") as log:
    print(("locked " if marker and Path(marker).exists() else "") + " ".join(arguments), file=log)
def used():
    return sum(data.stat().st_size for data in root.rglob("data")) // 1024
if arguments[:2] == ["volume", "inspect"]:
    sys.exit(0 if (root / "volumes" / arguments[2]).is_dir() else 1)
if arguments[:1] == ["info"]:
    print(8)
    sys.exit(0)
if arguments[:2] == ["buildx", "ls"]:
    print("default")
    if os.environ.get("FAKE_BUILDER_KIB"):
        print("junto-ci-deployment")
    sys.exit(0)
if arguments[:2] == ["buildx", "du"]:
    print(f"Reclaimable:\t1kB\nTotal:\t\t{int(os.environ['FAKE_BUILDER_KIB']) * 1024 / 1e9:.2f}GB")
    sys.exit(0)
if arguments[0] != "run":
    sys.exit(0)
volume = root / "volumes" / arguments[arguments.index("-v") + 1].split(":")[0]
volume.mkdir(parents=True, exist_ok=True)
command = arguments[arguments.index("-v") + 3:]
if command[0] == "find":
    for file in volume.rglob("*"):
        file.unlink()
elif command[0] == "du":
    print(f"{sum(file.stat().st_size for file in volume.rglob('*') if file.is_file()) // 1024}\t/cache")
elif command[0] == "df":
    if os.environ.get("FAKE_DF_FAILS"):
        sys.exit(1)
    disk = int(os.environ["FAKE_DISK"])
    print("Filesystem 1024-blocks Used Available Capacity Mounted on")
    print(f"disk {disk} {used()} {disk - used()} 1% /cache")
'''


def sparse(path, kib):
    path.mkdir(parents=True, exist_ok=True)
    with open(path / "data", "wb") as data:
        data.truncate(kib * 1024)


class Plan(unittest.TestCase):
    limits = {name: value * GIB for name, value in caches.LIMITS.items()}

    def test_a_cache_stays_when_the_run_leaves_enough_free_space(self):
        self.assertEqual(caches.plan(30 * GIB, [22 * GIB], self.limits, []), ("keep", 10 * GIB))

    def test_the_use_of_a_run_is_the_largest_recorded_drop_of_runs_in_the_same_state(self):
        drops = [(True, 5 * GIB), (True, 9 * GIB), (False, 30 * GIB)]
        self.assertEqual(caches.plan(30 * GIB, [22 * GIB], self.limits, drops), ("keep", 9 * GIB))
        self.assertEqual(caches.plan(28 * GIB, [22 * GIB], self.limits, drops), ("empty", 30 * GIB))

    def test_only_the_last_records_count(self):
        drops = [(True, 50 * GIB)] + [(True, 2 * GIB)] * caches.RECORDS
        self.assertEqual(caches.plan(25 * GIB, [22 * GIB], self.limits, drops), ("keep", 2 * GIB))

    # A run after the removal fills the caches again: without a record of such a run, emptying
    # them does not leave more free space at the end of the run.
    def test_without_a_record_of_a_run_without_caches_emptying_them_does_not_help(self):
        self.assertEqual(caches.plan(50 * GIB, [22 * GIB], self.limits, [(True, 40 * GIB)]),
                         ("refuse", 40 * GIB))

    def test_a_run_without_a_target_cache_is_judged_by_the_runs_without_it(self):
        self.assertEqual(caches.plan(45 * GIB, [0], self.limits, [(False, 25 * GIB)]), ("keep", 25 * GIB))
        self.assertEqual(caches.plan(44 * GIB, [0], self.limits, [(False, 25 * GIB)]), ("refuse", 25 * GIB))

    def test_the_history_pairs_the_records_of_each_run(self):
        with tempfile.TemporaryDirectory(prefix="junto-ci-history-") as temporary:
            caches.record(temporary, "a", "before", 100, 5)
            caches.record(temporary, "a", "after", 70, 9)
            caches.record(temporary, "b", "after", 10, 9)  # a run that started no job
            caches.record(temporary, "c", "before", 50, 0)
            caches.record(temporary, "c", "after", 60, 9)
            self.assertEqual(caches.history(temporary), [(True, 30), (False, 0)])


class Settings(unittest.TestCase):
    def read(self, text):
        with tempfile.TemporaryDirectory(prefix="junto-ci-settings-") as temporary:
            (Path(temporary) / "settings").write_bytes(text.encode())
            return caches.settings(temporary)

    def test_cr_lf_line_ends_and_leading_zeros_are_read(self):
        limits = self.read("JUNTO_CI_TARGET_CAP_GIB=120\r\nJUNTO_CI_FREE_GIB=08\r\n")
        self.assertEqual(limits["JUNTO_CI_TARGET_CAP_GIB"], 120 * GIB)
        self.assertEqual(limits["JUNTO_CI_FREE_GIB"], 8 * GIB)
        self.assertEqual(limits["JUNTO_CI_RUN_GIB"], 10 * GIB)

    def test_a_value_that_is_not_a_number_names_its_line(self):
        with self.assertRaisesRegex(caches.Failed, r"settings:2: JUNTO_CI_FREE_GIB is not a whole number of GiB: 60G"):
            self.read("JUNTO_CI_RUN_GIB=4\nJUNTO_CI_FREE_GIB=60G\n")

    def test_an_unknown_name_is_refused(self):
        with self.assertRaisesRegex(caches.Failed, r"settings:1: not a setting of caches.py: JUNTO_CI_FRE_GIB=6"):
            self.read("JUNTO_CI_FRE_GIB=6\n")


class Size(unittest.TestCase):
    def test_the_files_below_a_directory_are_summed_once_each_without_following_links(self):
        with tempfile.TemporaryDirectory(prefix="junto-ci-size-") as temporary:
            root = Path(temporary)
            (root / "outside").write_bytes(b"x" * 1024 * 1024)
            deep = root / "cache" / "debug" / "deps"
            deep.mkdir(parents=True)
            (deep / "first").write_bytes(b"x" * 3 * 1024)
            (root / "cache" / "debug" / "second").write_bytes(b"x" * 5 * 1024)
            os.link(deep / "first", root / "cache" / "debug" / "first")
            os.symlink(root / "outside", root / "cache" / "link")
            os.symlink(root, root / "cache" / "debug" / "loop")
            self.assertEqual(caches.directory_size(root / "cache"), 8)


class Fixture(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="junto-ci-caches-")
        self.root = Path(self.temporary.name)
        programs = self.root / "bin"
        programs.mkdir()
        (programs / "docker").write_text(DOCKER)
        (programs / "docker").chmod(0o755)
        self.directory = self.root / "ci"
        self.directory.mkdir()
        self.environment = dict(os.environ, PATH=f"{programs}:{os.environ['PATH']}", FAKE_ROOT=str(self.root),
                                FAKE_DISK=str(200 * GIB), JUNTO_CI_PYTHON_IMAGE="python-image")
        self.environment.pop("FAKE_BUILDER_KIB", None)

    def tearDown(self):
        self.temporary.cleanup()

    def settings(self, text):
        (self.directory / "settings").write_text(text)

    def caches(self, runner, phase, run="sample-run", **variables):
        return subprocess.run([sys.executable, str(CI / "caches.py"), str(self.directory), runner, phase, run],
                              env=dict(self.environment, **variables), capture_output=True, text=True, timeout=60)

    def log(self):
        return (self.root / "docker.log").read_text()


class Local(Fixture):
    def test_a_cache_above_its_cap_is_emptied_before_and_after_a_run(self):
        self.settings("JUNTO_CI_RUN_GIB=0\nJUNTO_CI_FREE_GIB=0\n")
        for phase in ("before", "after"):
            sparse(self.directory / "cargo-target", 61 * GIB)
            result = self.caches("macos", phase)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertFalse((self.directory / "cargo-target").exists(), phase)
            self.assertIn(f"{phase}: cache cargo-target: {61 * GIB} KiB, above its cap", result.stdout)

    def test_a_cache_within_its_cap_stays_and_the_run_is_recorded(self):
        self.settings("JUNTO_CI_RUN_GIB=0\nJUNTO_CI_FREE_GIB=0\n")
        sparse(self.directory / "cargo-target", 59 * GIB)
        sparse(self.directory / "cargo" / "registry", 1 * GIB)
        result = self.caches("windows", "before")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue((self.directory / "cargo-target").exists())
        self.assertIn(f"before: cache cargo/registry: {GIB} KiB of {8 * GIB} KiB", result.stdout)
        history = (self.directory / "cache-history.tsv").read_text().split("\t")
        self.assertEqual((history[0], history[1], history[3]), ("sample-run", "before", f"{59 * GIB}\n"))

    # The free space of this machine is real here; a level above it stands for a full disk.
    def test_a_run_that_would_leave_too_little_free_space_starts_no_job_and_keeps_the_caches(self):
        free = shutil.disk_usage(self.directory).free // 1024
        self.settings(f"JUNTO_CI_RUN_GIB=0\nJUNTO_CI_FREE_GIB={free // GIB + 100}\n")
        sparse(self.directory / "cargo-target", 10 * GIB)
        before = self.caches("macos", "before")
        self.assertEqual(before.returncode, 1, before.stdout + before.stderr)
        self.assertIn("the run starts no job: the disk of the caches has", before.stderr)
        self.assertTrue((self.directory / "cargo-target").exists())
        self.assertFalse((self.directory / "cache-history.tsv").exists())
        after = self.caches("macos", "after")
        self.assertEqual(after.returncode, 0, after.stdout + after.stderr)
        self.assertTrue((self.directory / "cargo-target").exists(), "the free space empties nothing after a run")

    def test_the_builder_of_the_deployment_job_is_a_cache_of_the_macos_machine(self):
        self.settings("JUNTO_CI_RUN_GIB=0\nJUNTO_CI_FREE_GIB=0\nJUNTO_CI_BUILDER_CAP_GIB=2\n")
        result = self.caches("macos", "after", FAKE_BUILDER_KIB=str(3 * GIB))
        self.assertEqual(result.returncode, 0, result.stderr)
        # docker buildx du states the size in GB with two decimals.
        self.assertRegex(result.stdout, r"cache builder junto-ci-deployment: 314\d{4} KiB, above its cap of 2097152 KiB; emptied")
        self.assertIn("buildx prune --builder junto-ci-deployment --all --force", self.log())
        absent = self.caches("macos", "after")
        self.assertNotIn("builder", absent.stdout)


class Linux(Fixture):
    def test_the_volumes_are_measured_and_emptied_in_containers(self):
        sparse(self.root / "volumes" / "junto-ci-target", 70 * GIB)
        sparse(self.root / "volumes" / "junto-ci-uv", 3 * GIB)
        result = self.caches("linux", "before")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse((self.root / "volumes" / "junto-ci-target" / "data").exists())
        self.assertTrue((self.root / "volumes" / "junto-ci-uv" / "data").exists())
        self.assertIn("volume inspect junto-ci-cargo", self.log())
        self.assertNotIn("cache junto-ci-cargo:", result.stdout, "a volume that does not exist")
        self.assertIn(f"cache junto-ci-uv: {3 * GIB} KiB of {4 * GIB} KiB", result.stdout)

    def test_the_caches_are_emptied_when_a_run_without_them_fits_and_the_run_with_them_does_not(self):
        sparse(self.root / "volumes" / "junto-ci-target", 30 * GIB)
        for run, free_before, free_after, target in (("warm", 100, 60, 1), ("cold", 100, 75, 0)):
            caches.record(str(self.directory), run, "before", free_before * GIB, target)
            caches.record(str(self.directory), run, "after", free_after * GIB, target)
        # 50 GiB free, a run with the cache uses 40, one without it 25, and 20 must remain.
        result = self.caches("linux", "before", FAKE_DISK=str(80 * GIB))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("cache junto-ci-target emptied", result.stdout)
        self.assertFalse((self.root / "volumes" / "junto-ci-target" / "data").exists())

    def test_a_free_space_that_cannot_be_read_stops_the_run_and_empties_nothing(self):
        sparse(self.root / "volumes" / "junto-ci-target", 30 * GIB)
        result = self.caches("linux", "before", FAKE_DF_FAILS="1", FAKE_DISK=str(31 * GIB))
        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        self.assertIn("caches.py: the free of junto-ci-target: exit status 1", result.stderr)
        self.assertTrue((self.root / "volumes" / "junto-ci-target" / "data").exists())


class Runner(Fixture):
    """run-jobs.sh of a commit that holds tools/ci and tools/windows of this working tree and a
    tools/serial.sh that creates LOCK_MARKER, runs the locked part without a lock, and removes
    the marker."""

    def run_jobs(self, runner, **variables):
        repository = self.root / "repository"
        (repository / "tools").mkdir(parents=True)
        shutil.copytree(CI, repository / "tools" / "ci", ignore=shutil.ignore_patterns("__pycache__"))
        shutil.copytree(CI.parent / "windows", repository / "tools" / "windows",
                        ignore=shutil.ignore_patterns("__pycache__"))
        (repository / "tools" / "serial.sh").write_text(
            '#!/bin/sh\n[ "$1" != --inherited ] || shift\n: >"$LOCK_MARKER"\n"$@"\nstatus=$?\n'
            'rm "$LOCK_MARKER"\nexit "$status"\n')
        identity = {"GIT_AUTHOR_NAME": "Example", "GIT_COMMITTER_NAME": "Example",
                    "GIT_AUTHOR_EMAIL": "ci@example.test", "GIT_COMMITTER_EMAIL": "ci@example.test"}
        git = ["git", "-C", str(repository)]
        subprocess.run(["git", "init", "--quiet", str(repository)], check=True)
        subprocess.run([*git, "add", "."], check=True)
        subprocess.run([*git, "commit", "--quiet", "-m", "Fixture"], env=dict(os.environ, **identity), check=True)
        subprocess.run([*git, "update-ref", "refs/ci/sample-run", "HEAD"], check=True)
        environment = dict(self.environment, LOCK_MARKER=str(self.root / "locked"), **variables)
        environment.pop("JUNTO_CI_LOCKED", None)
        return subprocess.run(["sh", str(CI / "run-jobs.sh"), str(self.directory), "sample-run", runner,
                               str(repository), "sample-job"], env=environment, capture_output=True,
                              text=True, timeout=120)

    def test_a_run_whose_disk_is_too_full_starts_no_job(self):
        sparse(self.root / "volumes" / "junto-ci-target", 30 * GIB)
        # 25 GiB free beside a cache of 30 GiB: a run with it would leave 15, one without it as well.
        result = self.run_jobs("linux", FAKE_DISK=str(55 * GIB))
        here = self.directory / "runs" / "sample-run"
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertEqual((here / "finished").read_text().strip(), "1")
        self.assertFalse((here / "record.tsv").exists())
        self.assertIn("the run starts no job", result.stderr)
        self.assertTrue((self.root / "volumes" / "junto-ci-target" / "data").exists())

    # Every measurement and removal of a cache happens while the run holds its lock.
    def test_the_caches_are_checked_before_the_first_job_and_after_the_last_with_the_lock_held(self):
        self.settings("JUNTO_CI_RUN_GIB=0\n")
        sparse(self.root / "volumes" / "junto-ci-target", 30 * GIB)
        result = self.run_jobs("linux")
        here = self.directory / "runs" / "sample-run"
        # The Linux runner has no job of this name: the job runs and fails.
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertEqual((here / "record.tsv").read_text().split("\t")[1], "sample-job")
        log = (here / "caches.log").read_text()
        self.assertLess(log.index("before: cache junto-ci-target"), log.index("after: cache junto-ci-target"))
        cache_calls = [line for line in self.log().splitlines()
                       if "junto-ci-cache-" in line or "volume inspect" in line]
        # Before and after: the four volumes inspected, one size, one free space.
        self.assertEqual(len(cache_calls), 2 * (4 + 2), cache_calls)
        self.assertEqual([call for call in cache_calls if not call.startswith("locked ")], [])


if __name__ == "__main__":
    unittest.main()
