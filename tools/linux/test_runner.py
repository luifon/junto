#!/usr/bin/env python3
"""Exercise runner failure paths without a container engine."""
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
import image_inputs  # noqa: E402

TOOLS = Path(__file__).resolve().parent.parent
SERIAL = Path(os.environ.get("JUNTO_SERIAL_UNDER_TEST", TOOLS / "serial.sh"))
COMMON = TOOLS.parent / "deploy/livekit-common.yaml"


class RunnerFailures(unittest.TestCase):
    def run_script(self, scenario, launcher=False, memory="6g", processors="4", cpu_first=None, service_first=None,
                   extra=None, during=None):
        """Runs the runner with the command `true`. `during(root, process)` is called while it runs."""
        with tempfile.TemporaryDirectory(prefix="junto-linux-runner-", dir="/tmp") as directory:
            root = Path(directory)
            tools = root / "checkout/tools"
            (tools / "linux").mkdir(parents=True)
            shutil.copy(os.environ.get("JUNTO_RUNNER_UNDER_TEST", TOOLS / "linux/run.sh"), tools / "linux/run.sh")
            shutil.copy(TOOLS / "linux-container.sh", tools / "linux-container.sh")
            shutil.copy(SERIAL, tools / "serial.sh")
            shutil.copy(TOOLS / "linux/build_jobs.py", tools / "linux/build_jobs.py")
            shutil.copy(TOOLS / "linux/cpu_sets.py", tools / "linux/cpu_sets.py")
            shutil.copy(TOOLS / "linux/image_inputs.py", tools / "linux/image_inputs.py")
            shutil.copy(TOOLS / "linux/Dockerfile", tools / "linux/Dockerfile")
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
    "image prune "*) echo "Error response from daemon: a prune operation is already running" >&2; exit 1 ;;
    "image inspect --format {{index "*)
        [ "$SCENARIO" = current_image ] || exit 1
        echo "$IMAGE_INPUTS"; exit 0 ;;
    "image inspect --format {{.Id}} "*) echo sha256:new; exit 0 ;;
    "image inspect --format {{.Parent}} sha256:new") echo sha256:parent; exit 0 ;;
    "image inspect --format {{.Parent}} "*) echo; exit 0 ;;
    "image ls "*) [ "$SCENARIO" = replaced_image ] && echo sha256:old sha256:new sha256:parent sha256:older; exit 0 ;;
    "run --name "*)
        if [ -d "$CHECKOUT/spikes/test-participant/target" ]; then echo present >> "$COMMAND_LOG.participant"; fi
        if [ "${SCENARIO#long_build}" != "$SCENARIO" ]; then
            # A build that runs until its container is removed, at most 60 seconds.
            echo "$$" > "$COMMAND_LOG.client.new" && mv "$COMMAND_LOG.client.new" "$COMMAND_LOG.client"
            tries=0
            while [ ! -e "$COMMAND_LOG.removed" ] && [ "$tries" -lt 600 ]; do sleep 0.1; tries=$((tries + 1)); done
            exit 137
        fi
        exit 37 ;;
    "rm -f -v junto-linux-build") touch "$COMMAND_LOG.removed" ;;
esac
exit 0
''')
            docker.chmod(0o755)
            for name, body in [("uname", 'if [ "$SCENARIO" = macos ]; then echo Darwin; else echo Linux; fi'), ("id", 'if [ "$SCENARIO" = root ]; then echo 0; else case "$1" in -u) echo 12345 ;; -g) echo 23456 ;; esac; fi'),
                               ("getconf", "echo 16"), ("flock", 'printf \'flock %s\\n\' "$1" >> "$COMMAND_LOG"; shift\n'
                                         # util-linux flock runs the command as its child.
                                         '"$@" &\nwait "$!"')]:
                executable = root / name
                executable.write_text(f"#!/bin/sh\n{body}\n")
                executable.chmod(0o755)
            search = f"{root}:{os.environ['PATH']}"
            if launcher:
                # A Linux host has flock and no lockf.
                for name in ["sh", "dirname", "nice", "grep", "mkdir", "cat", "python3", "sleep", "touch", "mv", "ps",
                             "tr"]:
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
            for name in ("JUNTO_LINUX_INSTANCE", "JUNTO_LINUX_IMAGE_BUILT", "JUNTO_LINUX_CPU_SET",
                         "JUNTO_LINUX_SERVICE_CPU_SET"):
                environment.pop(name, None)
            environment["IMAGE_INPUTS"] = image_inputs.key(tools / "linux", [
                "BUILD_UID=1000" if scenario in ("root", "macos") else "BUILD_UID=12345",
                "BUILD_GID=1000" if scenario in ("root", "macos") else "BUILD_GID=23456",
                "JUNTO_LINUX_IMAGE=junto-linux"])
            environment.update(extra or {})
            environment.pop("JUNTO_LINUX_POSTGRES_CPUS", None)
            environment.pop("JUNTO_LINUX_BUILDER", None)
            if scenario == "limited_builder":
                environment["JUNTO_LINUX_BUILDER"] = "limited"
            if scenario == "database_budget":
                environment["JUNTO_LINUX_POSTGRES_CPUS"] = "2.75"
            script = tools / ("linux-container.sh" if launcher else "linux/run.sh")
            process = subprocess.Popen(["/bin/sh", str(script), "true"], env=environment, stdout=subprocess.PIPE,
                                       stderr=subprocess.PIPE, text=True, start_new_session=True)
            try:
                if during:
                    during(root, process)
                stdout, stderr = process.communicate(timeout=120)
            finally:
                if process.poll() is None:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait()
            result = subprocess.CompletedProcess(process.args, process.returncode, stdout, stderr)
            commands = root / "commands"
            result.commands = commands.read_text() if commands.exists() else ""
            builder = root / "commands.builder"
            result.builder = builder.read_text().strip() if builder.exists() else None
            result.participant = (root / "commands.participant").exists()
            return result

    def test_an_artifact_failure_preserves_the_command_failure(self):
        result = self.run_script("artifact_failure")
        self.assertEqual(result.returncode, 37, result.stderr)

    def test_a_replaced_image_is_removed_without_a_prune_of_the_daemon(self):
        # The daemon refuses a prune while another one runs on the host. Only the images of this
        # prefix that are not the new image or one of its parents go, each without its parents:
        # with the containerd image store a removal with parents also removes a parent that
        # another image still has.
        result = self.run_script("replaced_image")
        self.assertEqual(result.returncode, 37, result.stderr)
        self.assertIn("image ls -a -q --no-trunc --filter label=junto-linux-image=junto-linux\n", result.commands)
        self.assertIn("image rm --no-prune sha256:old sha256:older\n", result.commands)
        self.assertNotIn("image prune", result.commands)

    def test_an_image_with_the_inputs_of_the_checkout_is_not_built_again(self):
        result = self.run_script("current_image")
        self.assertEqual(result.returncode, 37, result.stderr)
        self.assertFalse([line for line in result.commands.splitlines() if line.startswith("build ")])
        self.assertNotIn("image rm", result.commands)
        self.assertIn("not built", result.stdout)

    def test_an_image_with_other_inputs_is_built_with_the_key_and_the_prefix_of_its_chain(self):
        result = self.run_script("ownership")
        self.assertEqual(result.returncode, 37, result.stderr)
        image = next(line for line in result.commands.splitlines() if line.startswith("build "))
        self.assertIn(" --label junto-linux-inputs=", image)
        self.assertIn(" --build-arg JUNTO_LINUX_IMAGE=junto-linux ", image)

    def test_a_command_that_must_not_build_fails_when_the_image_has_other_inputs(self):
        result = self.run_script("ownership", extra={"JUNTO_LINUX_IMAGE_BUILT": "1"})
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("forbids a build", result.stderr)
        self.assertFalse([line for line in result.commands.splitlines() if line.startswith(("run ", "build "))])
        result = self.run_script("current_image", extra={"JUNTO_LINUX_IMAGE_BUILT": "1"})
        self.assertEqual(result.returncode, 37, result.stderr)

    def test_an_instance_names_the_containers_and_shares_the_image_and_volumes(self):
        result = self.run_script("current_image", extra={"JUNTO_LINUX_INSTANCE": "api"})
        self.assertEqual(result.returncode, 37, result.stderr)
        self.assertIn("network create junto-linux-api-tests", result.commands)
        for name in ("postgres", "livekit"):
            self.assertIn(f"run -d --name junto-linux-api-{name} ", result.commands)
        build = next(line for line in result.commands.splitlines() if line.startswith("run --name junto-linux-api-build "))
        self.assertIn("src=junto-linux-target,dst=/target", build)
        self.assertIn(" junto-linux-build:1 ", build)
        result = self.run_script("current_image", extra={"JUNTO_LINUX_INSTANCE": "a b"})
        self.assertEqual(result.returncode, 2, result.stderr)

    def test_a_command_without_services_starts_only_its_build_container(self):
        result = self.run_script("current_image", memory="4g",
                                 extra={"JUNTO_LINUX_SERVICES": "none", "JUNTO_LINUX_CARGO_JOBS": "1"})
        self.assertEqual(result.returncode, 37, result.stderr)
        self.assertNotIn("run -d ", result.commands)
        build = next(line for line in result.commands.splitlines() if line.startswith("run --name junto-linux-build "))
        self.assertIn(" -e JUNTO_LINUX_SERVICES=none ", build)
        self.assertIn(" -e CARGO_BUILD_JOBS=1 ", build)
        for name, value in [("JUNTO_LINUX_SERVICES", "some"), ("JUNTO_LINUX_CARGO_JOBS", "0"),
                            ("JUNTO_LINUX_CARGO_JOBS", "two")]:
            with self.subTest(name=name, value=value):
                result = self.run_script("current_image", extra={name: value})
                self.assertEqual(result.returncode, 2, result.stderr)
                self.assertIn(name, result.stderr)

    def test_the_caller_can_give_both_processor_sets_as_lists(self):
        result = self.run_script("current_image", memory="14g",
                                 extra={"JUNTO_LINUX_CPU_SET": "0-1,4-5,8-11", "JUNTO_LINUX_SERVICE_CPU_SET": "12-15"})
        self.assertEqual(result.returncode, 37, result.stderr)
        build = next(line for line in result.commands.splitlines() if line.startswith("run --name junto-linux-build "))
        self.assertIn(" --cpuset-cpus 0-1,4-5,8-11 ", build)
        self.assertIn(" -e CARGO_BUILD_JOBS=4 ", build)
        postgres = next(line for line in result.commands.splitlines() if line.startswith("run -d --name junto-linux-postgres "))
        self.assertIn(" --cpuset-cpus 12-15 ", postgres)
        result = self.run_script("current_image", extra={"JUNTO_LINUX_CPU_SET": "8-16"})
        self.assertEqual(result.returncode, 2, result.stderr)
        self.assertIn("CPU", result.stderr)

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

    # The runner was ended by a signal or by its caller while the build container ran: the
    # container and the network are removed, and the client of the container and the runner
    # end (CD-07).
    def end_a_long_build(self, launcher, end):
        ended = {}

        def during(root, process):
            client = root / "commands.client"
            deadline = time.monotonic() + 60
            while not client.exists():
                self.assertLess(time.monotonic(), deadline, "the build container did not start")
                self.assertIsNone(process.poll(), "the runner ended before its build container started")
                time.sleep(0.05)
            runner = process.pid if launcher is None else children(process.pid)[0]
            ended["client"] = int(client.read_text())
            ended["runner"] = runner
            began = time.monotonic()
            end(process, runner)
            deadline = began + 20
            while time.monotonic() < deadline and (
                    "network rm junto-linux-tests" not in (root / "commands").read_text()
                    or alive(ended["client"]) or alive(runner)):
                time.sleep(0.05)
            ended["seconds"] = time.monotonic() - began

        result = self.run_script("long_build", launcher=launcher is not None, extra={"JUNTO_LINUX_SERVICES": "none"},
                                 during=during)
        self.assertIn("rm -f -v junto-linux-build", result.commands, result.stderr)
        self.assertIn("network rm junto-linux-tests", result.commands, result.stderr)
        self.assertFalse(alive(ended["client"]), "the client of the build container still runs")
        self.assertFalse(alive(ended["runner"]), "the runner still runs")
        self.assertLess(ended["seconds"], 20, result.stderr)

    def test_a_term_to_the_runner_removes_the_build_container_at_once(self):
        self.end_a_long_build(None, lambda process, _: process.send_signal(signal.SIGTERM))

    @unittest.skipUnless("flock" in SERIAL.read_text(), "the lock script has no flock path yet")
    def test_a_term_to_the_lock_program_ends_the_runner_and_removes_its_container(self):
        # The caller ends the process that it started, the lock program, whose child is the runner.
        self.end_a_long_build("lock", lambda process, _: process.send_signal(signal.SIGTERM))

    @unittest.skipUnless("flock" in SERIAL.read_text(), "the lock script has no flock path yet")
    def test_a_kill_of_the_runner_still_removes_its_container(self):
        self.end_a_long_build("lock", lambda _, runner: os.kill(runner, signal.SIGKILL))

    @unittest.skipUnless("flock" in SERIAL.read_text(), "the lock script has no flock path yet")
    def test_the_launcher_runs_on_a_host_without_lockf(self):
        result = self.run_script("ownership", launcher=True)
        self.assertEqual(result.returncode, 37, result.stderr)
        self.assertIn("flock ", result.commands)
        self.assertIn("run --name junto-linux-build ", result.commands)


def children(pid):
    """The processes whose parent is `pid`."""
    listed = subprocess.run(["ps", "-A", "-o", "pid=,ppid="], capture_output=True, text=True).stdout.split()
    return [int(child) for child, parent in zip(listed[0::2], listed[1::2]) if int(parent) == pid]


def alive(pid):
    """Whether a process exists that is not a zombie."""
    result = subprocess.run(["ps", "-o", "stat=", "-p", str(pid)], capture_output=True, text=True)
    return result.returncode == 0 and not result.stdout.strip().startswith("Z")


if __name__ == "__main__":
    unittest.main()
