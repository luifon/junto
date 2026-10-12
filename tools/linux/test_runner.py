#!/usr/bin/env python3
"""Exercise runner failure paths without a container engine."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest


class RunnerFailures(unittest.TestCase):
    def run_script(self, scenario):
        with tempfile.TemporaryDirectory(prefix="junto-linux1-runner-") as directory:
            root = Path(directory)
            docker = root / "docker"
            docker.write_text('''#!/bin/sh
case "$*" in
    "container inspect "*) exit 1 ;;
    "network inspect "*) [ "$SCENARIO" = stale_network ]; exit $? ;;
    "network create "*) [ "$SCENARIO" != stale_network ]; exit $? ;;
    "cp "*) exit 1 ;;
    "run --name junto-linux1-build "*) exit 37 ;;
esac
exit 0
''')
            docker.chmod(0o755)
            environment = dict(os.environ, PATH=f"{root}:{os.environ['PATH']}",
                               SCENARIO=scenario, JUNTO_LINUX_ARTIFACTS=str(root / "artifacts"))
            return subprocess.run(["sh", str(Path(__file__).with_name("run.sh"))],
                                  env=environment, capture_output=True, text=True)

    def test_an_artifact_failure_preserves_the_command_failure(self):
        result = self.run_script("artifact_failure")
        self.assertEqual(result.returncode, 37, result.stderr)

    def test_a_leftover_network_has_an_actionable_error(self):
        result = self.run_script("stale_network")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("network junto-linux1-tests already exists", result.stderr)


if __name__ == "__main__":
    unittest.main()
