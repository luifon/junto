#!/usr/bin/env python3
"""Check that a runner checkout can be used inside a container."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

RUNNER = Path(__file__).with_name("run-jobs.sh")


class Checkout(unittest.TestCase):
    def test_the_checkout_does_not_need_the_hosts_object_directory(self):
        with tempfile.TemporaryDirectory(prefix="junto-ci-checkout-") as temporary:
            root = Path(temporary)
            repository = root / "repository"
            repository.mkdir()
            subprocess.run(["git", "init", "--quiet", str(repository)], check=True)
            tools = repository / "tools"
            tools.mkdir()
            serial = tools / "serial.sh"
            serial.write_text('''#!/bin/sh
set -u
mv "$SOURCE_REPOSITORY" "$SOURCE_REPOSITORY.hidden"
git -C "$RUN_CHECKOUT" diff --exit-code HEAD -- tracked.txt
status=$?
mv "$SOURCE_REPOSITORY.hidden" "$SOURCE_REPOSITORY"
exit "$status"
''')
            serial.chmod(0o755)
            (repository / "tracked.txt").write_text("content\n")
            git = ["git", "-C", str(repository)]
            subprocess.run([*git, "add", "."], check=True)
            identity = dict(os.environ, GIT_AUTHOR_NAME="Example", GIT_COMMITTER_NAME="Example",
                            GIT_AUTHOR_EMAIL="ci@example.test", GIT_COMMITTER_EMAIL="ci@example.test")
            subprocess.run([*git, "commit", "--quiet", "-m", "Fixture"], env=identity, check=True)
            subprocess.run([*git, "update-ref", "refs/ci/sample-run", "HEAD"], check=True)
            directory = root / "runner"
            checkout = directory / "runs/sample-run/checkout"
            environment = dict(os.environ, SOURCE_REPOSITORY=str(repository), RUN_CHECKOUT=str(checkout))
            environment.pop("JUNTO_CI_LOCKED", None)
            result = subprocess.run(["sh", RUNNER, directory, "sample-run", "linux", repository, "sample-job"],
                                    env=environment, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual((directory / "runs/sample-run/finished").read_text().strip(), "0")
            self.assertFalse(checkout.exists())


if __name__ == "__main__":
    unittest.main()
