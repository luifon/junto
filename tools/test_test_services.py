"""Check failure evidence and cleanup without starting containers."""

import os
from pathlib import Path
import subprocess
import tempfile
import unittest

TOOLS = Path(__file__).resolve().parent


@unittest.skipIf(os.name == "nt", "POSIX container wrappers")
class TestServiceLogs(unittest.TestCase):
    def run_wrapped(self, scripts, status, start_fails=False, logs_fail=False, settings=None):
        with tempfile.TemporaryDirectory(prefix="junto-service-logs-") as directory:
            root = Path(directory)
            docker = root / "docker"
            docker.write_text('''#!/bin/sh
printf '%s\\n' "$*" >> "$COMMAND_LOG"
case "$1" in
    run) [ "$START_FAILS" = 0 ] || exit 29 ;;
    port) echo '127.0.0.1:54321' ;;
    inspect) echo 'container state' ;;
    logs)
        echo "container stdout: $*"
        echo "container stderr: $*" >&2
        [ "$LOGS_FAIL" = 0 ] || exit 31 ;;
esac
exit 0
''')
            docker.chmod(0o755)
            curl = root / "curl"
            curl.write_text("#!/bin/sh\nexit 0\n")
            curl.chmod(0o755)
            environment = dict(
                os.environ,
                PATH=f"{root}:{os.environ['PATH']}",
                COMMAND_LOG=str(root / "commands"),
                START_FAILS=str(int(start_fails)),
                LOGS_FAIL=str(int(logs_fail)),
                JUNTO_TEST_POSTGRES_CONTAINER="junto-dev-postgres-calls",
                JUNTO_TEST_LIVEKIT_CONTAINER="junto-dev-livekit-calls",
                JUNTO_TEST_WEBHOOK_ADDRESS="127.0.0.1:47890",
                JUNTO_TEST_LIVEKIT_PORT="47880",
                JUNTO_TEST_LIVEKIT_TCP_PORT="47881",
                JUNTO_TEST_LIVEKIT_UDP_PORT="47882",
                JUNTO_TEST_LIVEKIT_API_KEY="test-key",
                JUNTO_TEST_LIVEKIT_API_SECRET="test-secret",
                **(settings or {}),
            )
            command = ["sh", "-c", f"exit {status}"]
            for script in reversed(scripts):
                command = ["sh", str(TOOLS / f"with-test-{script}.sh"), *command]
            result = subprocess.run(command, env=environment, capture_output=True, text=True)
            return result, (root / "commands").read_text().splitlines()

    def test_failed_tests_print_both_container_logs_before_removal(self):
        result, commands = self.run_wrapped(["postgres", "livekit"], 37)
        self.assertEqual(result.returncode, 37, result.stderr)
        for container in ["junto-dev-postgres-calls", "junto-dev-livekit-calls"]:
            self.assertIn(f"container stdout: logs --timestamps {container}", result.stderr)
            self.assertIn(f"container stderr: logs --timestamps {container}", result.stderr)
            logged = commands.index(f"logs --timestamps {container}")
            removed = max(i for i, command in enumerate(commands)
                          if command == f"rm -f -v {container}")
            self.assertLess(logged, removed)

    def test_success_removes_the_containers_without_printing_logs(self):
        result, commands = self.run_wrapped(["postgres", "livekit"], 0)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn("container stdout", result.stdout + result.stderr)
        for container in ["junto-dev-postgres-calls", "junto-dev-livekit-calls"]:
            self.assertEqual(commands.count(f"rm -f -v {container}"), 2)

    def test_a_log_failure_does_not_replace_the_test_failure_or_skip_cleanup(self):
        result, commands = self.run_wrapped(["postgres", "livekit"], 37, logs_fail=True)
        self.assertEqual(result.returncode, 37, result.stderr)
        for container in ["junto-dev-postgres-calls", "junto-dev-livekit-calls"]:
            self.assertIn(f"logs --timestamps {container}", commands)
            self.assertEqual(commands.count(f"rm -f -v {container}"), 2)

    def test_a_failed_start_also_prints_the_container_log(self):
        for script in ["postgres", "livekit"]:
            with self.subTest(script=script):
                result, commands = self.run_wrapped([script], 0, start_fails=True)
                self.assertEqual(result.returncode, 29, result.stderr)
                self.assertIn("container stdout", result.stderr)
                self.assertEqual(sum(command.startswith("rm -f -v ")
                                     for command in commands), 2)

    def test_the_server_always_has_the_common_settings_of_the_deployment(self):
        common = (TOOLS.parent / "deploy/livekit-common.yaml").read_text(encoding="utf-8").strip()
        result, commands = self.run_wrapped(["livekit"], 0)
        self.assertEqual(result.returncode, 0, result.stderr)
        started = "\n".join(commands)
        self.assertIn("LIVEKIT_CONFIG=", started)
        self.assertIn(common, started)

if __name__ == "__main__":
    unittest.main()
