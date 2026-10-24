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
    def run_script(self, scenario, launcher=False, memory="6g", processors="4", cpu_first=None, service_first=None):
        with tempfile.TemporaryDirectory(prefix="junto-linux-runner-", dir="/tmp") as directory:
            root = Path(directory)
            tools = root / "checkout/tools"
            (tools / "linux").mkdir(parents=True)
            shutil.copy(os.environ.get("JUNTO_RUNNER_UNDER_TEST", TOOLS / "linux/run.sh"), tools / "linux/run.sh")
            shutil.copy(TOOLS / "linux-container.sh", tools / "linux-container.sh")
            shutil.copy(SERIAL, tools / "serial.sh")
            shutil.copy(TOOLS / "linux/build_jobs.py", tools / "linux/build_jobs.py")
            shutil.copy(TOOLS / "linux/cpu_sets.py", tools / "linux/cpu_sets.py")
            (root / "checkout/deploy").mkdir()
            shutil.copy(COMMON, root / "checkout/deploy/livekit-common.yaml")
            docker = root / "docker"
            docker.write_text('''#!/bin/sh
printf '%s\\n' "$*" >> "$COMMAND_LOG"
case "$*" in
    "build "*) printf '%s\\n' "${DOCKER_BUILDKIT-unset}" > "$COMMAND_LOG.builder" ;;
    "container inspect "*) exit 1 ;;
    "network inspect "*) [ "$SCENARIO" = stale_network ]; exit $? ;;
    "info "*) if [ "$SCENARIO" = two_processors ]; then echo 2; else echo 16; fi; exit 0 ;;
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
                               ("getconf", "echo 16"), ("flock", 'printf \'flock %s\\n\' "$1" >> "$COMMAND_LOG"; shift; exec "$@"')]:
                executable = root / name
                executable.write_text(f"#!/bin/sh\n{body}\n")
                executable.chmod(0o755)
            search = f"{root}:{os.environ['PATH']}"
            if launcher:
                # A Linux host has flock and no lockf.
                for name in ["sh", "dirname", "nice", "grep", "mkdir", "cat", "python3"]:
                    (root / name).symlink_to(shutil.which(name))
                search = str(root)
            environment = dict(os.environ, PATH=search, CHECKOUT=str(root / "checkout"),
                               SCENARIO=scenario, JUNTO_LINUX_MEMORY=memory, JUNTO_LINUX_CPUS=processors,
                               JUNTO_LINUX_ARTIFACTS=str(root / "artifacts"),
                               COMMAND_LOG=str(root / "commands"),
                               JUNTO_DOCKER_SOCKET="/tmp/junto-linux-test-docker.sock",
                               # The lock script under test takes a file of this test,
                               # never the lock of the machine.
                               JUNTO_HEAVY_LOCK=str(root / "heavy.lock"))
            for name in ("JUNTO_LINUX_CPU_FIRST", "JUNTO_LINUX_SERVICE_CPU_FIRST"):
                environment.pop(name, None)
            if cpu_first is not None:
                environment["JUNTO_LINUX_CPU_FIRST"] = cpu_first
            if service_first is not None:
                environment["JUNTO_LINUX_SERVICE_CPU_FIRST"] = service_first
            environment.pop("JUNTO_LINUX_PREFIX", None)
            environment.pop("JUNTO_LINUX_POSTGRES_CPUS", None)
            environment.pop("JUNTO_LINUX_BUILDER", None)
            if scenario == "limited_builder":
                environment["JUNTO_LINUX_BUILDER"] = "limited"
            if scenario == "database_budget":
                environment["JUNTO_LINUX_POSTGRES_CPUS"] = "2.75"
            script = tools / ("linux-container.sh" if launcher else "linux/run.sh")
            result = subprocess.run(["/bin/sh", str(script), "true"], env=environment, capture_output=True, text=True)
            commands = root / "commands"
            result.commands = commands.read_text() if commands.exists() else ""
            builder = root / "commands.builder"
            result.builder = builder.read_text().strip() if builder.exists() else None
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
        self.assertIn(" -e CARGO_BUILD_JOBS=1 ", build[0])
        self.assertNotIn(" --cpus ", build[0])

    def test_the_caller_selects_disjoint_build_and_service_sets(self):
        result = self.run_script("limited_builder", cpu_first="8", service_first="12")
        self.assertEqual(result.returncode, 37, result.stderr)
        build = next(line for line in result.commands.splitlines() if line.startswith("run --name junto-linux-build "))
        self.assertIn(" --cpuset-cpus 8-11 ", build)
        image = next(line for line in result.commands.splitlines() if line.startswith("build "))
        self.assertIn(" --cpuset-cpus 8-11 ", image)
        self.assertEqual(result.builder, "0")
        for name in ("postgres", "livekit", "control"):
            service = next(line for line in result.commands.splitlines() if line.startswith(f"run -d --name junto-linux-{name} ") or line.startswith(f"run -d --user 0 --name junto-linux-{name} "))
            self.assertIn(" --cpuset-cpus 12-15 ", service)

    def test_default_launcher_bounds_the_image_builder(self):
        result = self.run_script("ownership", launcher=True, cpu_first="4")
        self.assertEqual(result.returncode, 37, result.stderr)
        image = next(line for line in result.commands.splitlines() if line.startswith("build "))
        self.assertIn(" --cpuset-cpus 4-7 ", image)
        self.assertIn(" --memory 6g --memory-swap 6g ", image)
        self.assertEqual(result.builder, "0")

    def test_an_invalid_cpu_set_starts_no_container(self):
        for first, service in [("13", None), ("16", None), ("-1", None), ("bad", None), ("8", "13")]:
            with self.subTest(first=first, service=service):
                result = self.run_script("ownership", cpu_first=first, service_first=service)
                self.assertEqual(result.returncode, 2, result.stderr)
                self.assertIn("CPU", result.stderr)
                self.assertFalse([line for line in result.commands.splitlines()
                                  if line.startswith(("run ", "build "))], result.commands)

    def test_the_database_cpu_budget_can_be_selected(self):
        result = self.run_script("database_budget")
        postgres = next(line for line in result.commands.splitlines() if line.startswith("run -d --name junto-linux-postgres "))
        self.assertIn(" --cpus 2.75 ", postgres)

    def test_a_machine_with_fewer_processors_runs_as_many_build_jobs(self):
        result = self.run_script("two_processors", memory="32g")
        build = [line for line in result.commands.splitlines() if line.startswith("run --name junto-linux-build ")]
        self.assertEqual(len(build), 1, result.commands)
        self.assertIn(" --cpuset-cpus 0-1 ", build[0])
        self.assertIn(" -e CARGO_BUILD_JOBS=2 ", build[0])

    def test_cargo_jobs_fit_the_memory_limit(self):
        for memory, processors, jobs in [("8g", "4", 2), ("8192m", "4", 2),
                                         ("8589934592", "4", 2), ("8G", "4", 2),
                                         ("8388608k", "4", 2), ("6g", "4", 1),
                                         ("5g", "4", 1), ("32g", "4", 4)]:
            with self.subTest(memory=memory, processors=processors):
                result = self.run_script("ownership", memory=memory, processors=processors)
                self.assertEqual(result.returncode, 37, result.stderr)
                self.assertIn(f" -e CARGO_BUILD_JOBS={jobs} ", result.commands)

    def test_too_little_memory_starts_no_container(self):
        result = self.run_script("ownership", memory="4g")
        self.assertEqual(result.returncode, 2, result.stderr)
        self.assertIn("at least 5 GiB", result.stderr)
        self.assertFalse([line for line in result.commands.splitlines()
                                  if line.startswith(("run ", "build "))], result.commands)

    def test_invalid_memory_starts_no_container(self):
        for memory in ["eight", "-8g", "8g extra"]:
            with self.subTest(memory=memory):
                result = self.run_script("ownership", memory=memory)
                self.assertEqual(result.returncode, 2, result.stderr)
                self.assertIn("JUNTO_LINUX_MEMORY", result.stderr)
                self.assertFalse([line for line in result.commands.splitlines()
                                  if line.startswith(("run ", "build "))], result.commands)

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
