#!/usr/bin/env python3
"""Exercise runner failure paths without a container engine."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest


class RunnerFailures(unittest.TestCase):
    def run_script(self, scenario):
        with tempfile.TemporaryDirectory(prefix="junto-linux1-runner-", dir="/tmp") as directory:
            root = Path(directory)
            docker = root / "docker"
            docker.write_text('''#!/bin/sh
printf '%s\\n' "$*" >> "$COMMAND_LOG"
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
            for name, body in [("uname", "echo Linux"), ("id", 'case "$1" in -u) echo 12345 ;; -g) echo 23456 ;; esac')]:
                executable = root / name
                executable.write_text(f"#!/bin/sh\n{body}\n")
                executable.chmod(0o755)
            environment = dict(os.environ, PATH=f"{root}:{os.environ['PATH']}",
                               SCENARIO=scenario, JUNTO_LINUX_ARTIFACTS=str(root / "artifacts"),
                               COMMAND_LOG=str(root / "commands"),
                               JUNTO_DOCKER_SOCKET="/tmp/junto-linux1-test-docker.sock")
            script = os.environ.get("JUNTO_RUNNER_UNDER_TEST", str(Path(__file__).with_name("run.sh")))
            result = subprocess.run(["sh", script], env=environment, capture_output=True, text=True)
            result.commands = (root / "commands").read_text()
            return result

    def test_an_artifact_failure_preserves_the_command_failure(self):
        result = self.run_script("artifact_failure")
        self.assertEqual(result.returncode, 37, result.stderr)

    def test_a_leftover_network_has_an_actionable_error(self):
        result = self.run_script("stale_network")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("network junto-linux1-tests already exists", result.stderr)

    def test_the_linux_source_owner_is_the_build_user(self):
        result = self.run_script("ownership")
        self.assertIn("--build-arg BUILD_UID=12345 --build-arg BUILD_GID=23456", result.commands)
        self.assertIn("JUNTO_BUILD_UID=12345", result.commands)
        self.assertIn("JUNTO_BUILD_GID=23456", result.commands)

    def test_a_rootless_docker_socket_can_be_selected(self):
        result = self.run_script("rootless")
        self.assertIn("src=/tmp/junto-linux1-test-docker.sock,dst=/var/run/docker.sock", result.commands)


if __name__ == "__main__":
    unittest.main()
