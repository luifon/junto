#!/usr/bin/env python3
"""Check that a runner checkout can be used inside a container and on Windows."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

RUNNER = Path(__file__).with_name("run-jobs.sh")
IDENTITY = {"GIT_AUTHOR_NAME": "Example", "GIT_COMMITTER_NAME": "Example",
            "GIT_AUTHOR_EMAIL": "ci@example.test", "GIT_COMMITTER_EMAIL": "ci@example.test"}
# The lock program of the fixture commit: it fails when the files of the checkout exist before
# it runs the locked part, and runs `check` after that part.
SERIAL = '''#!/bin/sh
if [ -e "$RUN_CHECKOUT/tracked.txt" ]; then
    echo "the files of the checkout exist before the lock" >&2
    exit 9
fi
"$@"
{check}
'''


def repository_with(root, check):
    """A repository whose ref refs/ci/sample-run holds tools/ci of this working tree, the lock
    program SERIAL with the shell command `check`, and a file tracked.txt with two lines."""
    repository = root / "repository"
    repository.mkdir()
    subprocess.run(["git", "init", "--quiet", str(repository)], check=True)
    tools = repository / "tools"
    shutil.copytree(RUNNER.parent, tools / "ci", ignore=shutil.ignore_patterns("__pycache__"))
    (tools / "serial.sh").write_text(SERIAL.format(check=check))
    (tools / "serial.sh").chmod(0o755)
    (repository / "tracked.txt").write_text("first line\nsecond line\n")
    git = ["git", "-C", str(repository)]
    subprocess.run([*git, "add", "."], check=True)
    subprocess.run([*git, "commit", "--quiet", "-m", "Fixture"], env=dict(os.environ, **IDENTITY), check=True)
    subprocess.run([*git, "update-ref", "refs/ci/sample-run", "HEAD"], check=True)
    return repository


def run(directory, source, environment):
    """Runs the job sample-job, which the macOS runner does not have, in a run of the macOS
    runner. A program named docker in the PATH lists no container, network, volume or image,
    and the caches have no free-space level."""
    environment = dict(environment)
    environment.pop("JUNTO_CI_LOCKED", None)
    programs = Path(directory).parent / "bin"
    programs.mkdir(exist_ok=True)
    (programs / "docker").write_text("#!/bin/sh\nexit 0\n")
    (programs / "docker").chmod(0o755)
    environment["PATH"] = f"{programs}:{environment['PATH']}"
    Path(directory).mkdir(parents=True, exist_ok=True)
    (Path(directory) / "settings").write_text("JUNTO_CI_FREE_GIB=0\n")
    return subprocess.run(["sh", RUNNER, directory, "sample-run", "macos", source, "sample-job"],
                          env=environment, capture_output=True, text=True)


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
        with tempfile.TemporaryDirectory(prefix="junto-ci-written-") as temporary:
            root = Path(temporary)
            repository = repository_with(root, 'grep -qx "first line" "$RUN_CHECKOUT/tracked.txt"')
            directory = root / "runner"
            checkout = directory / "runs/sample-run/checkout"
            result = run(directory, repository, dict(os.environ, RUN_CHECKOUT=str(checkout)))
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assert_ran(directory)
            self.assertFalse(checkout.exists())

    def test_the_checkout_does_not_need_the_hosts_object_directory(self):
        with tempfile.TemporaryDirectory(prefix="junto-ci-checkout-") as temporary:
            root = Path(temporary)
            repository = repository_with(root, '''set -u
mv "$SOURCE_REPOSITORY" "$SOURCE_REPOSITORY.hidden"
git -C "$RUN_CHECKOUT" diff --exit-code HEAD -- tracked.txt
status=$?
mv "$SOURCE_REPOSITORY.hidden" "$SOURCE_REPOSITORY"
exit "$status"
''')
            directory = root / "runner"
            checkout = directory / "runs/sample-run/checkout"
            result = run(directory, repository,
                         dict(os.environ, SOURCE_REPOSITORY=str(repository), RUN_CHECKOUT=str(checkout)))
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assert_ran(directory)
            self.assertFalse(checkout.exists())

    # The Windows runner receives the commit as a bundle file, which is all the checkout
    # reads; the bundle is removed with the checkout.
    def test_a_bundle_of_the_run_is_its_repository(self):
        with tempfile.TemporaryDirectory(prefix="junto-ci-bundle-") as temporary:
            root = Path(temporary)
            repository = repository_with(root, '''grep -qx 'second line' "$RUN_CHECKOUT/tracked.txt"
''')
            bundle = root / "commit.bundle"
            subprocess.run(["git", "-C", str(repository), "bundle", "create", "--quiet", str(bundle),
                            "refs/ci/sample-run"], check=True)
            subprocess.run(["rm", "-rf", str(repository)], check=True)
            directory = root / "runner"
            checkout = directory / "runs/sample-run/checkout"
            result = run(directory, bundle, dict(os.environ, RUN_CHECKOUT=str(checkout)))
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assert_ran(directory)
            self.assertFalse(checkout.exists())
            self.assertFalse(bundle.exists())

    # Git for Windows converts line ends to CR LF on checkout by its system settings; the
    # job scripts run in Bash and the tests compare files with those of the other systems.
    def test_the_checkout_keeps_the_line_ends_of_the_commit(self):
        with tempfile.TemporaryDirectory(prefix="junto-ci-line-ends-") as temporary:
            root = Path(temporary)
            repository = repository_with(root, '''! grep -q "$(printf '\\r')" "$RUN_CHECKOUT/tracked.txt" "$RUN_CHECKOUT/tools/serial.sh"
''')
            settings = root / "gitconfig"
            settings.write_text("[core]\n\tautocrlf = true\n")
            directory = root / "runner"
            checkout = directory / "runs/sample-run/checkout"
            result = run(directory, repository,
                         dict(os.environ, RUN_CHECKOUT=str(checkout), GIT_CONFIG_GLOBAL=str(settings)))
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assert_ran(directory)


if __name__ == "__main__":
    unittest.main()
