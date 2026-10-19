#!/usr/bin/env python3
"""Exercise runner failure paths without a container engine."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

TOOLS = Path(__file__).resolve().parent.parent
SERIAL = Path(os.environ.get("JUNTO_SERIAL_UNDER_TEST", TOOLS / "serial.sh"))
COMMON = TOOLS.parent / "deploy/livekit-common.yaml"


class RunnerFailures(unittest.TestCase):
    def run_script(self, scenario, launcher=False):
        with tempfile.TemporaryDirectory(prefix="junto-linux-runner-", dir="/tmp") as directory:
            root = Path(directory)
            tools = root / "checkout/tools"
            (tools / "linux").mkdir(parents=True)
            shutil.copy(os.environ.get("JUNTO_RUNNER_UNDER_TEST", TOOLS / "linux/run.sh"), tools / "linux/run.sh")
            shutil.copy(TOOLS / "linux-container.sh", tools / "linux-container.sh")
            shutil.copy(SERIAL, tools / "serial.sh")
            (root / "checkout/deploy").mkdir()
            shutil.copy(COMMON, root / "checkout/deploy/livekit-common.yaml")
            docker = root / "docker"
            docker.write_text('''#!/bin/sh
printf '%s\\n' "$*" >> "$COMMAND_LOG"
case "$*" in
    "container inspect "*) exit 1 ;;
    "network inspect "*) [ "$SCENARIO" = stale_network ]; exit $? ;;
    "info "*) if [ "$SCENARIO" = two_processors ]; then echo 2; fi; exit 0 ;;
    "network create "*) [ "$SCENARIO" != stale_network ]; exit $? ;;
    "cp "*) exit 1 ;;
    "run --name junto-linux-build "*)
        if [ -d "$CHECKOUT/spikes/test-participant/target" ]; then echo present >> "$COMMAND_LOG.participant"; fi
        exit 37 ;;
esac
exit 0
''')
            docker.chmod(0o755)
            for name, body in [("uname", 'if [ "$SCENARIO" = macos ]; then echo Darwin; else echo Linux; fi'), ("id", 'if [ "$SCENARIO" = root ]; then echo 0; else case "$1" in -u) echo 12345 ;; -g) echo 23456 ;; esac; fi'),
                               ("flock", 'printf \'flock %s\\n\' "$1" >> "$COMMAND_LOG"; shift; exec "$@"')]:
                executable = root / name
                executable.write_text(f"#!/bin/sh\n{body}\n")
                executable.chmod(0o755)
            search = f"{root}:{os.environ['PATH']}"
            if launcher:
                # A Linux host has flock and no lockf.
                for name in ["sh", "dirname", "nice", "grep", "mkdir", "cat"]:
                    (root / name).symlink_to(shutil.which(name))
                search = str(root)
            environment = dict(os.environ, PATH=search, CHECKOUT=str(root / "checkout"),
                               SCENARIO=scenario, JUNTO_LINUX_ARTIFACTS=str(root / "artifacts"),
                               COMMAND_LOG=str(root / "commands"),
                               JUNTO_DOCKER_SOCKET="/tmp/junto-linux-test-docker.sock",
                               # The lock script under test takes a file of this test,
                               # never the lock of the machine.
                               JUNTO_HEAVY_LOCK=str(root / "heavy.lock"))
            environment.pop("JUNTO_LINUX_PREFIX", None)
            environment.pop("JUNTO_LINUX_POSTGRES_CPUS", None)
            if scenario == "database_budget":
                environment["JUNTO_LINUX_POSTGRES_CPUS"] = "2.75"
            script = tools / ("linux-container.sh" if launcher else "linux/run.sh")
            result = subprocess.run(["/bin/sh", str(script), "true"], env=environment, capture_output=True, text=True)
            commands = root / "commands"
            result.commands = commands.read_text() if commands.exists() else ""
            result.participant = (root / "commands.participant").exists()
            return result

    def test_an_artifact_failure_preserves_the_command_failure(self):
        result = self.run_script("artifact_failure")
        self.assertEqual(result.returncode, 37, result.stderr)

    def test_a_leftover_network_has_an_actionable_error(self):
        result = self.run_script("stale_network")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("network junto-linux-tests already exists", result.stderr)

    def test_the_linux_source_owner_is_the_build_user(self):
        result = self.run_script("ownership")
        self.assertIn("--build-arg BUILD_UID=12345 --build-arg BUILD_GID=23456", result.commands)
        self.assertIn("run-as-user.sh 12345 23456 /target /usr/local/cargo/registry --", result.commands)

    def test_a_root_caller_runs_tests_as_user_1000(self):
        result = self.run_script("root")
        self.assertIn("--build-arg BUILD_UID=1000 --build-arg BUILD_GID=1000", result.commands)
        build = next(line for line in result.commands.splitlines() if line.startswith("run --name junto-linux-build "))
        self.assertIn("/opt/junto-linux/run-as-user.sh 1000 1000 /src /target /usr/local/cargo/registry --", build)

    def test_macos_does_not_change_bind_mount_ownership(self):
        result = self.run_script("macos")
        self.assertIn("run-as-user.sh 1000 1000 /target /usr/local/cargo/registry --", result.commands)
        self.assertNotIn("run-as-user.sh 1000 1000 /src ", result.commands)

    def test_a_rootless_docker_socket_can_be_selected(self):
        result = self.run_script("rootless")
        self.assertIn("src=/tmp/junto-linux-test-docker.sock,dst=/var/run/docker.sock", result.commands)

    def test_the_invoking_user_creates_the_participant_mount_point(self):
        # Docker would otherwise create the missing directory as root on a Linux host.
        result = self.run_script("ownership")
        self.assertTrue(result.participant, result.commands)

    def test_the_build_container_gets_processors_as_a_set_and_not_as_a_quota(self):
        # A quota stops every thread of the container for the rest of each 100 ms in
        # which it is used up; a test that measures sound then measures those stops.
        result = self.run_script("ownership")
        build = [line for line in result.commands.splitlines() if line.startswith("run --name junto-linux-build ")]
        self.assertEqual(len(build), 1, result.commands)
        self.assertIn(" --cpuset-cpus 0-3 ", build[0])
        self.assertIn(" -e CARGO_BUILD_JOBS=4 ", build[0])
        self.assertNotIn(" --cpus ", build[0])

    def test_the_database_cpu_budget_can_be_selected(self):
        result = self.run_script("database_budget")
        postgres = next(line for line in result.commands.splitlines() if line.startswith("run -d --name junto-linux-postgres "))
        self.assertIn(" --cpus 2.75 ", postgres)

    def test_a_machine_with_fewer_processors_runs_as_many_build_jobs(self):
        result = self.run_script("two_processors")
        build = [line for line in result.commands.splitlines() if line.startswith("run --name junto-linux-build ")]
        self.assertEqual(len(build), 1, result.commands)
        self.assertIn(" --cpuset-cpus 0-1 ", build[0])
        self.assertIn(" -e CARGO_BUILD_JOBS=2 ", build[0])

    def test_the_media_server_has_the_common_settings_of_the_deployment(self):
        result = self.run_script("ownership")
        start = result.commands.index("run -d --name junto-linux-livekit ")
        end = result.commands.index("--dev --bind 0.0.0.0", start)
        self.assertIn(COMMON.read_text(encoding="utf-8").strip(), result.commands[start:end])

    @unittest.skipUnless("flock" in SERIAL.read_text(), "the lock script has no flock path yet")
    def test_the_launcher_runs_on_a_host_without_lockf(self):
        result = self.run_script("ownership", launcher=True)
        self.assertEqual(result.returncode, 37, result.stderr)
        self.assertIn("flock ", result.commands)
        self.assertIn("run --name junto-linux-build ", result.commands)


if __name__ == "__main__":
    unittest.main()
