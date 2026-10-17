#!/usr/bin/env python3
"""Check that a runner checkout can be used inside a container and on Windows."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

RUNNER = Path(__file__).with_name("run-jobs.sh")
IDENTITY = {"GIT_AUTHOR_NAME": "Example", "GIT_COMMITTER_NAME": "Example",
            "GIT_AUTHOR_EMAIL": "ci@example.test", "GIT_COMMITTER_EMAIL": "ci@example.test"}


def repository_with(root, serial):
    """A repository whose ref refs/ci/sample-run holds tools/serial.sh with the text `serial`
    and a file tracked.txt with two lines."""
    repository = root / "repository"
    repository.mkdir()
    subprocess.run(["git", "init", "--quiet", str(repository)], check=True)
    tools = repository / "tools"
    tools.mkdir()
    (tools / "serial.sh").write_text(serial)
    (tools / "serial.sh").chmod(0o755)
    (repository / "tracked.txt").write_text("first line\nsecond line\n")
    git = ["git", "-C", str(repository)]
    subprocess.run([*git, "add", "."], check=True)
    subprocess.run([*git, "commit", "--quiet", "-m", "Fixture"], env=dict(os.environ, **IDENTITY), check=True)
    subprocess.run([*git, "update-ref", "refs/ci/sample-run", "HEAD"], check=True)
    return repository


def run(directory, source, environment):
    environment = dict(environment)
    environment.pop("JUNTO_CI_LOCKED", None)
    return subprocess.run(["sh", RUNNER, directory, "sample-run", "linux", source, "sample-job"],
                          env=environment, capture_output=True, text=True)


class Checkout(unittest.TestCase):
    def test_the_checkout_does_not_need_the_hosts_object_directory(self):
        with tempfile.TemporaryDirectory(prefix="junto-ci-checkout-") as temporary:
            root = Path(temporary)
            repository = repository_with(root, '''#!/bin/sh
set -u
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
            self.assertEqual((directory / "runs/sample-run/finished").read_text().strip(), "0")
            self.assertFalse(checkout.exists())

    # The Windows runner receives the commit as a bundle file, which is all the checkout
    # reads; the bundle is removed with the checkout.
    def test_a_bundle_of_the_run_is_its_repository(self):
        with tempfile.TemporaryDirectory(prefix="junto-ci-bundle-") as temporary:
            root = Path(temporary)
            repository = repository_with(root, '''#!/bin/sh
grep -qx 'second line' "$RUN_CHECKOUT/tracked.txt"
''')
            bundle = root / "commit.bundle"
            subprocess.run(["git", "-C", str(repository), "bundle", "create", "--quiet", str(bundle),
                            "refs/ci/sample-run"], check=True)
            subprocess.run(["rm", "-rf", str(repository)], check=True)
            directory = root / "runner"
            checkout = directory / "runs/sample-run/checkout"
            result = run(directory, bundle, dict(os.environ, RUN_CHECKOUT=str(checkout)))
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual((directory / "runs/sample-run/finished").read_text().strip(), "0")
            self.assertFalse(checkout.exists())
            self.assertFalse(bundle.exists())

    # Git for Windows converts line ends to CR LF on checkout by its system settings; the
    # job scripts run in Bash and the tests compare files with those of the other systems.
    def test_the_checkout_keeps_the_line_ends_of_the_commit(self):
        with tempfile.TemporaryDirectory(prefix="junto-ci-line-ends-") as temporary:
            root = Path(temporary)
            repository = repository_with(root, '''#!/bin/sh
! grep -q "$(printf '\\r')" "$RUN_CHECKOUT/tracked.txt" "$RUN_CHECKOUT/tools/serial.sh"
''')
            settings = root / "gitconfig"
            settings.write_text("[core]\n\tautocrlf = true\n")
            directory = root / "runner"
            checkout = directory / "runs/sample-run/checkout"
            result = run(directory, repository,
                         dict(os.environ, RUN_CHECKOUT=str(checkout), GIT_CONFIG_GLOBAL=str(settings)))
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual((directory / "runs/sample-run/finished").read_text().strip(), "0")


if __name__ == "__main__":
    unittest.main()
