#!/usr/bin/env python3
"""Tests of tools/ci/caches.sh, tools/ci/cache_size.py and of their use by tools/ci/run-jobs.sh.
A cache is a directory with a sparse file named data, of the size of the cache. Programs named
df and docker in the PATH stand for one disk of FAKE_DISK KiB, whose free space is the disk less
the sizes of the files named data below FAKE_ROOT; docker runs du in place of the container."""
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

CI = Path(__file__).resolve().parent
GIB = 1024 * 1024
FAKES = {
    "du": r'''#!/usr/bin/env python3
import sys
from pathlib import Path
path = Path(sys.argv[-1])
print(f"{sum(file.stat().st_size for file in path.rglob('*') if file.is_file()) // 1024}\t{path}")
''',
    "df": r'''#!/usr/bin/env python3
import os
from pathlib import Path
used = sum(data.stat().st_size for data in Path(os.environ["FAKE_ROOT"]).rglob("data")) // 1024
print("Filesystem 1024-blocks Used Available Capacity Mounted on")
print(f"disk {os.environ['FAKE_DISK']} {used} {int(os.environ['FAKE_DISK']) - used} 1% /")
''',
    # docker volume inspect <name>; docker run ... -v <name>:/cache <image> <command>...;
    # docker ps, network ls, volume ls and images of the sweep of run-jobs.sh list nothing.
    "docker": r'''#!/usr/bin/env python3
import os
from pathlib import Path
import subprocess
import sys
root = Path(os.environ["FAKE_ROOT"])
arguments = sys.argv[1:]
with open(root / "docker.log", "a") as log:
    print(" ".join(arguments), file=log)
if arguments[:2] == ["volume", "inspect"]:
    sys.exit(0 if (root / "volumes" / arguments[2]).is_dir() else 1)
if arguments[0] != "run":
    sys.exit(0)
volume = root / "volumes" / arguments[arguments.index("-v") + 1].split(":")[0]
volume.mkdir(parents=True, exist_ok=True)
command = arguments[arguments.index("-v") + 3:]
if command[0] == "find":
    for file in volume.rglob("*"):
        file.unlink()
else:
    sys.exit(subprocess.run([command[0], command[1], str(volume)]).returncode)
''',
}


class Fixture(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="junto-ci-caches-")
        self.root = Path(self.temporary.name)
        programs = self.root / "bin"
        programs.mkdir()
        for name, text in FAKES.items():
            (programs / name).write_text(text)
            (programs / name).chmod(0o755)
        self.directory = self.root / "ci"
        self.directory.mkdir()
        self.environment = dict(os.environ, PATH=f"{programs}:{os.environ['PATH']}",
                                FAKE_ROOT=str(self.root), FAKE_DISK=str(200 * GIB))

    def tearDown(self):
        self.temporary.cleanup()

    def cache(self, name, gib, volume=False):
        path = (self.root / "volumes" if volume else self.directory) / name
        path.mkdir(parents=True)
        with open(path / "data", "wb") as data:
            data.truncate(gib * GIB * 1024)
        return path

    def caches(self, runner, phase, disk_gib=200):
        return subprocess.run(["sh", str(CI / "caches.sh"), str(self.directory), runner, phase],
                              env=dict(self.environment, FAKE_DISK=str(disk_gib * GIB)),
                              capture_output=True, text=True, timeout=60)


class Caches(Fixture):
    def test_a_cache_within_its_cap_on_a_disk_above_the_level_is_kept(self):
        target = self.cache("cargo-target", 59)
        result = self.caches("macos", "before")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(target.exists())
        self.assertIn(f"before: cache cargo-target: {59 * GIB} KiB of {60 * GIB} KiB", result.stdout)
        self.assertIn(f"before: free space: {141 * GIB} KiB, level {60 * GIB} KiB", result.stdout)

    def test_a_cache_above_its_cap_is_emptied_before_and_after_a_run(self):
        for phase in ("before", "after"):
            target = self.cache("cargo-target", 61)
            result = self.caches("macos", phase)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertFalse(target.exists(), phase)
            self.assertIn(f"{phase}: cache cargo-target: {61 * GIB} KiB, above its cap", result.stdout)

    def test_a_cache_within_its_cap_is_emptied_while_the_free_space_is_below_the_level(self):
        target = self.cache("cargo-target", 30)
        result = self.caches("macos", "before", disk_gib=80)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(target.exists())
        self.assertIn(f"cache cargo-target emptied for free space; free space: {80 * GIB} KiB", result.stdout)

    def test_the_largest_cache_is_emptied_first_and_only_as_many_as_the_level_needs(self):
        target = self.cache("cargo-target", 40)
        registry = self.cache("cargo/registry", 5)
        result = self.caches("windows", "before", disk_gib=100)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(target.exists())
        self.assertTrue(registry.exists())

    def test_a_run_does_not_start_while_the_free_space_stays_below_the_level(self):
        self.cache("cargo-target", 10)
        self.cache("../other", 150)
        before = self.caches("macos", "before", disk_gib=200)
        self.assertEqual(before.returncode, 1, before.stdout + before.stderr)
        self.assertIn(f"the run starts no job: the disk of the caches has {50 * GIB} KiB free", before.stderr)
        self.assertFalse((self.directory / "cargo-target").exists())
        after = self.caches("macos", "after", disk_gib=200)
        self.assertEqual(after.returncode, 0, after.stdout + after.stderr)

    def test_the_settings_file_of_the_ci_directory_sets_the_limits(self):
        target = self.cache("cargo-target", 30)
        (self.directory / "settings").write_text("JUNTO_CI_TARGET_CAP_GIB=20\nJUNTO_CI_FREE_GIB=0\n")
        result = self.caches("macos", "before", disk_gib=31)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(target.exists())
        (self.directory / "settings").write_text("JUNTO_CI_FREE_GIB=60G\n")
        refused = self.caches("macos", "before")
        self.assertEqual(refused.returncode, 2)
        self.assertIn("JUNTO_CI_FREE_GIB is not a whole number of GiB", refused.stderr)

    def test_the_volumes_of_the_linux_host_are_measured_and_emptied_in_containers(self):
        target = self.cache("junto-ci-target", 70, volume=True)
        uv = self.cache("junto-ci-uv", 3, volume=True)
        result = self.caches("linux", "before")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse((target / "data").exists())
        self.assertTrue((uv / "data").exists())
        self.assertIn("volume inspect junto-ci-cargo", (self.root / "docker.log").read_text())
        self.assertNotIn("cache junto-ci-cargo:", result.stdout, "a volume that does not exist")
        self.assertIn(f"cache junto-ci-uv: {3 * GIB} KiB of {4 * GIB} KiB", result.stdout)


class CacheSize(unittest.TestCase):
    def size(self, path):
        result = subprocess.run([sys.executable, str(CI / "cache_size.py"), str(path)],
                                capture_output=True, text=True, timeout=60)
        self.assertEqual(result.returncode, 0, result.stderr)
        return int(result.stdout)

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
            self.assertEqual(self.size(root / "cache"), 8)


class Runner(Fixture):
    """run-jobs.sh of a commit that holds tools/ci of this working tree and a tools/serial.sh
    that runs the locked part without a lock."""

    def run_jobs(self, disk_gib):
        repository = self.root / "repository"
        (repository / "tools").mkdir(parents=True)
        shutil.copytree(CI, repository / "tools" / "ci", ignore=shutil.ignore_patterns("__pycache__"))
        (repository / "tools" / "serial.sh").write_text('#!/bin/sh\nexec "$@"\n')
        (repository / "tools" / "serial.sh").chmod(0o755)
        identity = {"GIT_AUTHOR_NAME": "Example", "GIT_COMMITTER_NAME": "Example",
                    "GIT_AUTHOR_EMAIL": "ci@example.test", "GIT_COMMITTER_EMAIL": "ci@example.test"}
        git = ["git", "-C", str(repository)]
        subprocess.run(["git", "init", "--quiet", str(repository)], check=True)
        subprocess.run([*git, "add", "."], check=True)
        subprocess.run([*git, "commit", "--quiet", "-m", "Fixture"], env=dict(os.environ, **identity), check=True)
        subprocess.run([*git, "update-ref", "refs/ci/sample-run", "HEAD"], check=True)
        environment = dict(self.environment, FAKE_DISK=str(disk_gib * GIB))
        environment.pop("JUNTO_CI_LOCKED", None)
        return subprocess.run(["sh", str(CI / "run-jobs.sh"), str(self.directory), "sample-run", "macos",
                               str(repository), "sample-job"], env=environment, capture_output=True,
                              text=True, timeout=120)

    def test_a_run_on_a_disk_below_the_level_starts_no_job(self):
        self.cache("cargo-target", 30)
        result = self.run_jobs(disk_gib=50)
        here = self.directory / "runs" / "sample-run"
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertEqual((here / "finished").read_text().strip(), "1")
        self.assertFalse((here / "record.tsv").exists())
        self.assertIn("the run starts no job", result.stderr)
        self.assertFalse((self.directory / "cargo-target").exists())

    def test_a_run_checks_its_caches_before_its_first_job_and_after_its_last(self):
        self.cache("cargo-target", 30)
        result = self.run_jobs(disk_gib=200)
        here = self.directory / "runs" / "sample-run"
        # The macOS runner has no job of this name: the job runs and fails.
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertEqual((here / "record.tsv").read_text().split("\t")[1], "sample-job")
        log = (here / "caches.log").read_text()
        self.assertLess(log.index("before: cache cargo-target"), log.index("after: cache cargo-target"))
        self.assertTrue((self.directory / "cargo-target").exists())


if __name__ == "__main__":
    unittest.main()
