#!/usr/bin/env python3
"""Tests of the runner tools/ci/run-jobs.sh: its checkout can be used inside a container and on
Windows, its files are written after the run lock is taken, and a run that is ended by a signal
ends every process of its job before it checks the caches and releases the lock."""
import fcntl
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import tempfile
import time
import unittest

RUNNER = Path(__file__).with_name("run-jobs.sh")
TOOLS = RUNNER.parents[1]
IDENTITY = {"GIT_AUTHOR_NAME": "Example", "GIT_COMMITTER_NAME": "Example",
            "GIT_AUTHOR_EMAIL": "ci@example.test", "GIT_COMMITTER_EMAIL": "ci@example.test"}
# The lock program of the fixture commit: it fails when the files of the checkout exist before
# it runs the locked part, and runs `check` after that part.
SERIAL = """#!/bin/sh
[ "$1" != --inherited ] || shift
[ -z "${{SERIAL_RECORD:-}}" ] || echo "$0" >"$SERIAL_RECORD"
if [ -e "$RUN_CHECKOUT/tracked.txt" ]; then
    echo "the files of the checkout exist before the lock" >&2
    exit 9
fi
"$@"
{check}
"""
# Programs in place of those of the machine. docker states 8 processors, lists no container,
# network, volume or image, and keeps its volumes in directories below FAKE_ROOT/volumes on a
# disk with 1000 GiB free. cygpath of Git for Windows prints the path it is given.
DOCKER = """#!/usr/bin/env python3
import os, sys
from pathlib import Path
root = Path(os.environ["FAKE_ROOT"]) / "volumes"
arguments = sys.argv[1:]
if arguments[:1] == ["info"]:
    print(8)
elif arguments[:2] == ["volume", "inspect"]:
    sys.exit(0 if (root / arguments[2]).is_dir() else 1)
elif arguments[:1] == ["run"] and "-v" in arguments:
    volume = root / arguments[arguments.index("-v") + 1].split(":")[0]
    volume.mkdir(parents=True, exist_ok=True)
    command = arguments[arguments.index("-v") + 3:]
    files = [file for file in volume.rglob("*") if file.is_file()]
    if command[0] == "du":
        print(f"{sum(file.stat().st_size for file in files) // 1024}\t/cache")
    elif command[0] == "df":
        print("Filesystem 1024-blocks Used Available Capacity Mounted on")
        print("disk 1048576000 0 1048576000 0% /cache")
    elif command[0] == "find":
        for file in files:
            file.unlink()
"""
FAKES = {"docker": DOCKER, "cygpath": '#!/bin/sh\neval "last=\\${$#}"\necho "$last"\n'}


def repository_with(root, serial, files=None):
    """A repository whose ref refs/ci/sample-run holds tools/ci and tools/windows of this working
    tree, the lock program `serial`, a file tracked.txt with two lines, and `files`."""
    repository = root / "repository"
    repository.mkdir()
    subprocess.run(["git", "init", "--quiet", str(repository)], check=True)
    tools = repository / "tools"
    shutil.copytree(RUNNER.parent, tools / "ci", ignore=shutil.ignore_patterns("__pycache__"))
    shutil.copytree(TOOLS / "windows", tools / "windows", ignore=shutil.ignore_patterns("__pycache__"))
    (tools / "serial.sh").write_text(serial)
    (tools / "serial.sh").chmod(0o755)
    (repository / "tracked.txt").write_text("first line\nsecond line\n")
    for name, text in (files or {}).items():
        (repository / name).write_text(text)
        (repository / name).chmod(0o755)
    git = ["git", "-C", str(repository)]
    subprocess.run([*git, "add", "."], check=True)
    subprocess.run([*git, "commit", "--quiet", "-m", "Fixture"], env=dict(os.environ, **IDENTITY), check=True)
    subprocess.run([*git, "update-ref", "refs/ci/sample-run", "HEAD"], check=True)
    return repository


def checked(root, check):
    return repository_with(root, SERIAL.format(check=check))


def prepared(directory, environment, settings="JUNTO_CI_FREE_GIB=0\nJUNTO_CI_RUN_GIB=0\n"):
    """The environment of a run with the programs of FAKES, and the settings of its CI directory:
    by default the caches have no free-space level."""
    environment = dict(environment)
    environment.pop("JUNTO_CI_LOCKED", None)
    programs = Path(directory).parent / "bin"
    programs.mkdir(exist_ok=True)
    for name, text in FAKES.items():
        (programs / name).write_text(text)
        (programs / name).chmod(0o755)
    environment["PATH"] = f"{programs}:{environment['PATH']}"
    environment["FAKE_ROOT"] = str(Path(directory).parent)
    Path(directory).mkdir(parents=True, exist_ok=True)
    (Path(directory) / "settings").write_text(settings)
    return environment


def run(directory, source, environment, runner="macos"):
    """Runs the job sample-job, which no runner has."""
    return subprocess.run(["sh", RUNNER, directory, "sample-run", runner, source, "sample-job"],
                          env=prepared(directory, environment), capture_output=True, text=True, timeout=120)


def appeared(path, seconds=60):
    """Waits until a file exists."""
    deadline = time.monotonic() + seconds
    while not path.exists():
        if time.monotonic() > deadline:
            raise AssertionError(f"{path} did not appear")
        time.sleep(0.05)


class Checkout(unittest.TestCase):
    def assert_ran(self, directory):
        """The locked part ran the job in the checkout and recorded it."""
        record = (directory / "runs/sample-run/record.tsv").read_text().split("\t")
        self.assertEqual(record[1], "sample-job")
        self.assertTrue((directory / "runs/sample-run/finished").exists())
        self.assertFalse((directory / "runs/sample-run/lock").exists())

    # Cargo keeps the output of a unit whose source files are older than that output. The files
    # of a run that waited for the lock while an earlier run built would be older than its
    # output (ADR 0905).
    def test_the_files_of_the_checkout_are_written_after_the_lock_is_taken(self):
        for runner in ("linux", "macos", "windows"):
            with self.subTest(runner=runner), tempfile.TemporaryDirectory(prefix="junto-ci-written-") as temporary:
                root = Path(temporary)
                repository = checked(root, 'grep -qx "first line" "$RUN_CHECKOUT/tracked.txt"')
                directory = root / "runner"
                checkout = directory / "checkout"
                result = run(directory, repository, dict(os.environ, RUN_CHECKOUT=str(checkout),
                                                         SERIAL_RECORD=str(root / "serial")), runner)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assert_ran(directory)
                self.assertFalse((directory / "runs/sample-run/commit.git").exists())
                # The lock program of the commit was read from the repository of the run.
                self.assertEqual((root / "serial").read_text().strip(),
                                 f"{directory}/runs/sample-run/lock/tools/serial.sh")

    def test_the_checkout_does_not_need_the_hosts_object_directory(self):
        with tempfile.TemporaryDirectory(prefix="junto-ci-checkout-") as temporary:
            root = Path(temporary)
            repository = checked(root, '''set -u
mv "$SOURCE_REPOSITORY" "$SOURCE_REPOSITORY.hidden"
git -C "$RUN_CHECKOUT" diff --exit-code HEAD -- tracked.txt
status=$?
mv "$SOURCE_REPOSITORY.hidden" "$SOURCE_REPOSITORY"
exit "$status"
''')
            directory = root / "runner"
            checkout = directory / "checkout"
            result = run(directory, repository,
                         dict(os.environ, SOURCE_REPOSITORY=str(repository), RUN_CHECKOUT=str(checkout)))
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assert_ran(directory)
            self.assertTrue(checkout.exists(), "the checkout stays for the next run")

    # The Windows runner receives the commit as a bundle file, which is all the checkout
    # reads; the bundle is removed with the checkout.
    def test_a_bundle_of_the_run_is_its_repository(self):
        with tempfile.TemporaryDirectory(prefix="junto-ci-bundle-") as temporary:
            root = Path(temporary)
            repository = checked(root, '''grep -qx 'second line' "$RUN_CHECKOUT/tracked.txt"
''')
            bundle = root / "commit.bundle"
            subprocess.run(["git", "-C", str(repository), "bundle", "create", "--quiet", str(bundle),
                            "refs/ci/sample-run"], check=True)
            subprocess.run(["rm", "-rf", str(repository)], check=True)
            directory = root / "runner"
            checkout = directory / "checkout"
            result = run(directory, bundle, dict(os.environ, RUN_CHECKOUT=str(checkout)))
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assert_ran(directory)
            self.assertTrue(checkout.exists(), "the checkout stays for the next run")
            self.assertFalse(bundle.exists())

    # Git for Windows converts line ends to CR LF on checkout by its system settings; the
    # job scripts run in Bash and the tests compare files with those of the other systems.
    def test_the_checkout_keeps_the_line_ends_of_the_commit(self):
        with tempfile.TemporaryDirectory(prefix="junto-ci-line-ends-") as temporary:
            root = Path(temporary)
            repository = checked(root, '''! grep -q "$(printf '\\r')" "$RUN_CHECKOUT/tracked.txt" "$RUN_CHECKOUT/tools/serial.sh"
''')
            settings = root / "gitconfig"
            settings.write_text("[core]\n\tautocrlf = true\n")
            directory = root / "runner"
            checkout = directory / "checkout"
            result = run(directory, repository,
                         dict(os.environ, RUN_CHECKOUT=str(checkout), GIT_CONFIG_GLOBAL=str(settings)))
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assert_ran(directory)


# The job of the fixture commit, in place of the scripts of the Rust jobs: it starts a child and
# ends at TERM. The child ends one second after TERM; at its end it states whether the run lock
# and another lock file are held, and writes 2 MiB into the target cache. Both write their
# process numbers to JOB_PIDS, and both end by themselves after 120 seconds.
JOB = """#!/usr/bin/env python3
import os, subprocess, sys, time
CHILD = '''
import fcntl, os, signal, sys, time
from pathlib import Path
def held(path):
    with open(path, "a") as lock:
        try:
            fcntl.flock(lock.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
            return "free"
        except BlockingIOError:
            return "held"
def ending(*_):
    time.sleep(1)
    state = held(os.environ["RUN_LOCK"]) + " " + held(os.environ["OTHER_LOCK"])
    target = Path(os.environ["JOB_TARGET"])
    target.mkdir(parents=True, exist_ok=True)
    with open(target / "data", "wb") as data:
        data.truncate(2 * 1024 * 1024)
    Path(os.environ["JOB_ENDED"]).write_text(state)
    sys.exit(0)
signal.signal(signal.SIGTERM, ending)
with open(os.environ["JOB_PIDS"], "a") as pids:
    print(os.getpid(), file=pids)
Path(os.environ["JOB_STARTED"]).write_text("")
for _ in range(1200):
    time.sleep(0.1)
'''
with open(os.environ["JOB_PIDS"], "a") as pids:
    print(os.getpid(), file=pids)
subprocess.Popen([sys.executable, "-c", CHILD])
time.sleep(120)
"""


def end_left_processes(path):
    """Ends the processes that the fixture job recorded, when the runner left them running."""
    if path.exists():
        for number in path.read_text().split():
            try:
                os.kill(int(number), signal.SIGKILL)
            except ProcessLookupError:
                pass


class Ended(unittest.TestCase):
    """A run ended by a signal, with tools/serial.sh and tools/windows/lock.py of this working tree
    as its lock programs. A JUNTO_HEAVY_LOCK of the caller names another lock file, which the run
    must not take."""

    def end_a_run(self, runner, job, target, whole_group=False):
        with tempfile.TemporaryDirectory(prefix="junto-ci-ended-") as temporary:
            root = Path(temporary)
            self.addCleanup(end_left_processes, root / "pids")
            directory = root / "runner"
            files = {"tools/ci/rust-linux.sh": JOB, "tools/ci/rust-other-systems.sh": JOB}
            repository = repository_with(root, (TOOLS / "serial.sh").read_text(), files)
            lock = directory / "run.lock" if runner == "linux" else root / "machine.lock"
            environment = prepared(directory, dict(
                os.environ, JUNTO_HEAVY_LOCK=str(root / "other.lock"), OTHER_LOCK=str(root / "other.lock"),
                JUNTO_CI_RUN_LOCK=str(lock), RUN_LOCK=str(lock), JOB_PIDS=str(root / "pids"),
                JOB_TARGET=str(target(root, directory)), JOB_STARTED=str(root / "started"),
                JOB_ENDED=str(root / "ended")),
                "JUNTO_CI_FREE_GIB=0\nJUNTO_CI_RUN_GIB=0\nJUNTO_CI_TARGET_CAP_GIB=0\n")
            process = subprocess.Popen(["sh", RUNNER, directory, "sample-run", runner, repository, job],
                                       env=environment, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
                                       start_new_session=whole_group)
            try:
                appeared(root / "started")
                if whole_group:
                    # Ctrl-C in a terminal: INT to every process of the foreground group.
                    os.killpg(process.pid, signal.SIGINT)
                else:
                    process.send_signal(signal.SIGTERM)
                output, _ = process.communicate(timeout=120)
            finally:
                process.kill()
            here = directory / "runs" / "sample-run"
            self.assertEqual(process.returncode, 143, output)
            self.assertEqual((root / "ended").read_text(), "held free",
                             "the run lock was free, or the lock of JUNTO_HEAVY_LOCK was taken, while the job ran")
            # The check after the run saw the file that the child wrote at its end.
            self.assertRegex((here / "caches.log").read_text(),
                             r"after: cache [a-z-]+: 2048 KiB, above its cap of 0 KiB")
            self.assertEqual((here / "finished").read_text(), "143\n")
            self.assertTrue((directory / "checkout" / "tracked.txt").exists())
            self.assertFalse((here / "commit.git").exists())
            self.assertFalse((here / "lock").exists())
            with open(lock, "a") as file:
                fcntl.flock(file.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)

    def test_a_linux_run_ended_by_a_signal_ends_its_job_before_the_caches_and_the_lock(self):
        self.end_a_run("linux", "rust-linux", lambda root, _: root / "volumes" / "junto-ci-target")

    # BSD lockf frees its lock when its direct child ends; the run takes the lock with lock.py.
    def test_a_macos_run_ended_by_a_signal_ends_its_job_before_the_caches_and_the_lock(self):
        self.end_a_run("macos", "rust-macos", lambda _, directory: directory / "cargo-target")

    # Each process of the runner receives the INT, the locked part twice (also from lock.py).
    def test_a_ctrl_c_ends_the_job_and_still_checks_the_caches(self):
        self.end_a_run("macos", "rust-macos", lambda _, directory: directory / "cargo-target", whole_group=True)


# The job of the fixture commit for the macOS runner: it records its directory, the times and
# text of two files, and whether a file of an earlier run is there, and then leaves a file.
RECORDING = '''#!/usr/bin/env python3
import json, os
from pathlib import Path
record = {"path": str(Path.cwd().resolve()), "tracked": os.stat("tracked.txt").st_mtime_ns,
          "changed": os.stat("changed.txt").st_mtime_ns, "text": Path("changed.txt").read_text(),
          "left": Path("left.txt").exists()}
with open(os.environ["RUN_RECORD"], "a") as file:
    file.write(json.dumps(record) + "\\n")
Path("left.txt").write_text("written by a run")
'''


class FixedCheckout(unittest.TestCase):
    # Kept Cargo output names the checkout that built it (CARGO_MANIFEST_DIR); with a checkout
    # per run that path no longer existed in the next run (ADR 0905).
    def test_every_run_uses_one_checkout_and_rewrites_only_the_files_that_differ(self):
        with tempfile.TemporaryDirectory(prefix="junto-ci-fixed-") as temporary:
            root = Path(temporary)
            directory = root / "runner"
            repository = repository_with(root, (TOOLS / "serial.sh").read_text(),
                                         {"tools/ci/rust-other-systems.sh": RECORDING, "changed.txt": "first\n"})
            git = ["git", "-C", str(repository)]
            (repository / "changed.txt").write_text("second\n")
            subprocess.run([*git, "commit", "--quiet", "-am", "Second"], env=dict(os.environ, **IDENTITY), check=True)
            subprocess.run([*git, "update-ref", "refs/ci/second-run", "HEAD"], check=True)
            environment = prepared(directory, dict(os.environ, JUNTO_CI_RUN_LOCK=str(root / "machine.lock"),
                                                   RUN_RECORD=str(root / "record")))
            for run in ("sample-run", "second-run"):
                result = subprocess.run(["sh", RUNNER, directory, run, "macos", repository, "rust-macos"],
                                        env=environment, capture_output=True, text=True, timeout=120)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            first, second = [json.loads(line) for line in (root / "record").read_text().splitlines()]
            self.assertEqual(first["path"], second["path"])
            self.assertEqual(Path(first["path"]), (directory / "checkout").resolve())
            self.assertEqual(first["tracked"], second["tracked"], "an unchanged file was written again")
            self.assertGreater(second["changed"], first["changed"])
            self.assertEqual((first["text"], second["text"]), ("first\n", "second\n"))
            self.assertEqual((first["left"], second["left"]), (False, False), "a file of the earlier run stayed")


class Recovery(unittest.TestCase):
    # A run that is ended while Git writes the checkout (a stop of Windows, a KILL) leaves a lock
    # file of Git; with one checkout every later run failed at the checkout.
    def test_a_lock_file_or_a_damaged_repository_of_an_interrupted_run_does_not_stop_later_runs(self):
        with tempfile.TemporaryDirectory(prefix="junto-ci-recover-") as temporary:
            root = Path(temporary)
            directory = root / "runner"
            repository = repository_with(root, (TOOLS / "serial.sh").read_text(),
                                         {"tools/ci/rust-other-systems.sh": RECORDING, "changed.txt": "first\n"})
            git = ["git", "-C", str(repository)]
            for run in ("second-run", "third-run"):
                subprocess.run([*git, "update-ref", f"refs/ci/{run}", "HEAD"], check=True)
            environment = prepared(directory, dict(os.environ, JUNTO_CI_RUN_LOCK=str(root / "machine.lock"),
                                                   RUN_RECORD=str(root / "record")))
            checkout = directory / "checkout"
            for run, damage in (("sample-run", None), ("second-run", "index.lock"), ("third-run", "HEAD")):
                if damage == "index.lock":
                    (checkout / ".git" / "index.lock").write_text("")
                elif damage == "HEAD":
                    (checkout / ".git" / "HEAD").write_text("not a reference\n")
                result = subprocess.run(["sh", RUNNER, directory, run, "macos", repository, "rust-macos"],
                                        env=environment, capture_output=True, text=True, timeout=120)
                self.assertEqual(result.returncode, 0, (run, result.stdout + result.stderr))
                runner_log = (directory / "runs" / run / "runner.log")
                if damage == "HEAD":
                    self.assertIn("the checkout could not be updated; it is created again",
                                  result.stderr + (runner_log.read_text() if runner_log.exists() else ""))
            self.assertEqual(len((root / "record").read_text().splitlines()), 3)


# Git, with the index of the working tree of "git -C <directory> status" dated one hour ahead.
LATER_INDEX = """#!/usr/bin/env python3
import os, sys, time
arguments = sys.argv[1:]
if "status" in arguments and "-C" in arguments:
    index = os.path.join(arguments[arguments.index("-C") + 1], ".git", "index")
    if os.path.exists(index):
        later = time.time() + 3600
        os.utime(index, (later, later))
os.execv({git!r}, [{git!r}, *arguments])
"""


class CheckoutFailure(unittest.TestCase):
    """A failed Git step of the checkout stops the run or makes it create the checkout again; the
    jobs never test the files of another commit under the name of this one."""

    def two_commits(self, root, second_files):
        repository = repository_with(root, (TOOLS / "serial.sh").read_text(),
                                     {"tools/ci/rust-other-systems.sh": RECORDING, "changed.txt": "first\n"})
        git = ["git", "-C", str(repository)]
        (repository / "changed.txt").write_text("second\n")
        for name, data in second_files.items():
            (repository / name).write_bytes(data)
        subprocess.run([*git, "add", "-A"], check=True)
        subprocess.run([*git, "commit", "--quiet", "-m", "Second"], env=dict(os.environ, **IDENTITY), check=True)
        subprocess.run([*git, "update-ref", "refs/ci/second-run", "HEAD"], check=True)
        return repository

    def run_both(self, root, repository, between, fakes=None):
        directory = root / "runner"
        environment = prepared(directory, dict(os.environ, JUNTO_CI_RUN_LOCK=str(root / "machine.lock"),
                                               RUN_RECORD=str(root / "record")))
        for name, text in (fakes or {}).items():
            (root / "bin" / name).write_text(text)
            (root / "bin" / name).chmod(0o755)
        results = []
        for run in ("sample-run", "second-run"):
            if run == "second-run":
                between(directory / "checkout")
            results.append(subprocess.run(["sh", RUNNER, directory, run, "macos", repository, "rust-macos"],
                                          env=environment, capture_output=True, text=True, timeout=120))
        return directory, results

    # A power loss can leave an index that Git cannot read: "git checkout" and "git clean" then
    # fail, while "git gc --auto", the last step, succeeds.
    def test_a_checkout_that_fails_before_the_last_step_is_created_again(self):
        with tempfile.TemporaryDirectory(prefix="junto-ci-failed-") as temporary:
            root = Path(temporary)
            repository = self.two_commits(root, {})

            def cut_index(checkout):
                with open(checkout / ".git" / "index", "r+b") as index:
                    index.truncate(20)

            directory, results = self.run_both(root, repository, cut_index)
            for result in results:
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertIn("the checkout could not be updated; it is created again", results[1].stderr)
            second = json.loads((root / "record").read_text().splitlines()[1])
            self.assertEqual(second["text"], "second\n", "the job tested the files of the first commit")
            head = subprocess.run(["git", "-C", str(directory / "checkout"), "rev-parse", "HEAD"],
                                  capture_output=True, text=True).stdout.strip()
            self.assertEqual(head, (directory / "runs" / "second-run" / "commit").read_text().strip())

    # A file that Git reports as changed when it compares the content: the jobs would not test
    # the files of the commit. Git compares the content of a file whose recorded time is not
    # older than the index, and otherwise trusts the time; the wrapper dates the index one hour
    # ahead before "git status", as a checkout that crossed the start of a second leaves it.
    def test_a_checkout_that_differs_from_the_commit_starts_no_job(self):
        with tempfile.TemporaryDirectory(prefix="junto-ci-differs-") as temporary:
            root = Path(temporary)
            repository = self.two_commits(root, {})
            git = ["git", "-C", str(repository)]
            # Committed with CR LF line ends, then declared as text: Git sees the file as changed.
            (repository / "lines.txt").write_bytes(b"one\r\ntwo\r\n")
            subprocess.run([*git, "add", "lines.txt"], check=True)
            subprocess.run([*git, "commit", "--quiet", "-m", "Lines"], env=dict(os.environ, **IDENTITY), check=True)
            (repository / ".gitattributes").write_text("lines.txt text\n")
            subprocess.run([*git, "add", ".gitattributes"], check=True)
            subprocess.run([*git, "commit", "--quiet", "-m", "Text"], env=dict(os.environ, **IDENTITY), check=True)
            subprocess.run([*git, "update-ref", "refs/ci/second-run", "HEAD"], check=True)
            directory, results = self.run_both(root, repository, lambda _: None,
                                               {"git": LATER_INDEX.format(git=shutil.which("git"))})
            self.assertEqual(results[0].returncode, 0, results[0].stdout + results[0].stderr)
            self.assertEqual(results[1].returncode, 1, results[1].stdout + results[1].stderr)
            self.assertIn("the checkout differs from the commit of the run", results[1].stderr)
            self.assertIn("lines.txt", results[1].stderr)
            self.assertFalse((directory / "runs" / "second-run" / "record.tsv").exists())


if __name__ == "__main__":
    unittest.main()
