#!/usr/bin/env python3
"""Tests of tools/ci/budget.py, tools/ci/lanes.py and tools/ci/cargo_targets.py: the budget of a
step of the Linux job from the free worker locks, the lanes that run at the same time, and the
test programs of the lane of the other tests."""
import fcntl
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import budget  # noqa: E402
import cargo_targets  # noqa: E402
import lanes  # noqa: E402

GIB = budget.GIB
BASE = {"JUNTO_LINUX_CPU_FIRST": "8", "JUNTO_LINUX_CPUS": "4", "JUNTO_LINUX_SERVICE_CPU_FIRST": "12",
        "JUNTO_LINUX_MEMORY": "8g", "JUNTO_CI_HOST_CPUS": "16"}


class Budget(unittest.TestCase):
    def setUp(self):
        if os.name == "nt":
            self.skipTest("flock locks of the Linux host")
        self.directory = tempfile.TemporaryDirectory(prefix="junto-budget-")
        self.addCleanup(self.directory.cleanup)
        root = Path(self.directory.name)
        self.locks = [root / f"heavy-{first}.lock" for first in (0, 2, 4, 6)]
        self.environment = dict(BASE, JUNTO_CI_WORKER_LOCKS=" ".join(
            f"{path}:{first}" for path, first in zip(self.locks, (0, 2, 4, 6))))

    def hold(self, path):
        descriptor = os.open(path, os.O_RDWR | os.O_CREAT)
        fcntl.flock(descriptor, fcntl.LOCK_EX)
        self.addCleanup(os.close, descriptor)

    def is_free(self, path):
        descriptor = os.open(path, os.O_RDWR | os.O_CREAT)
        try:
            fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
            return True
        except OSError:
            return False
        finally:
            os.close(descriptor)

    def decide(self, commands=1, available=22 * GIB, idle=0.9, environment=None):
        base = budget.base_from(environment or self.environment)
        result = budget.decide(base, commands, environment or self.environment,
                               available=lambda: available, idle=lambda: idle)
        self.addCleanup(budget.release, result.locks)
        return result

    def test_the_free_worker_pairs_are_added_to_the_build_processors(self):
        self.hold(self.locks[1])
        result = self.decide()
        self.assertEqual(budget.cpu_text(result.build), "0-1,4-11")
        self.assertEqual(budget.cpu_text(result.services), "12-15")
        # 8 GiB and 6 GiB per pair, bounded by 22 GiB less 4 GiB and the services of one command.
        self.assertEqual(result.memory, 22 * GIB - budget.RESERVE - budget.SERVICES)
        self.assertEqual(len(result.locks), 3)
        self.assertFalse(self.is_free(self.locks[0]))
        budget.release(result.locks)
        self.assertTrue(self.is_free(self.locks[0]))

    def test_the_memory_grows_by_two_cargo_jobs_per_pair_when_memory_allows(self):
        for lock in self.locks[1:]:
            self.hold(lock)
        result = self.decide(available=30 * GIB)
        self.assertEqual(budget.cpu_text(result.build), "0-1,8-11")
        self.assertEqual(result.memory, 14 * GIB)

    def test_with_every_worker_lock_held_the_step_keeps_the_base_budget(self):
        for lock in self.locks:
            self.hold(lock)
        result = self.decide()
        self.assertEqual((budget.cpu_text(result.build), result.memory, result.locks), ("8-11", 8 * GIB, []))
        self.assertIn("every worker lock is held", budget.describe(result))

    def test_the_rules_of_an_extra_worker_lock_apply(self):
        for available, idle, reason in [(9 * GIB, 0.9, "available"), (22 * GIB, 0.2, "idle")]:
            with self.subTest(reason=reason):
                result = self.decide(available=available, idle=idle)
                self.assertEqual((budget.cpu_text(result.build), result.memory), ("8-11", 8 * GIB))
                self.assertIn(reason, result.reason)
                self.assertTrue(all(self.is_free(lock) for lock in self.locks))

    def test_without_memory_above_the_base_the_locks_are_released(self):
        # 14 GiB available less 4 GiB and the services of two commands leaves less than 8 GiB.
        result = self.decide(commands=2, available=14 * GIB)
        self.assertEqual((budget.cpu_text(result.build), result.memory, result.locks), ("8-11", 8 * GIB, []))
        self.assertTrue(all(self.is_free(lock) for lock in self.locks))

    def test_a_smaller_host_and_a_job_without_the_base_keep_their_budget(self):
        result = self.decide(environment=dict(self.environment, JUNTO_CI_HOST_CPUS="8",
                                              JUNTO_LINUX_CPU_FIRST="0", JUNTO_LINUX_SERVICE_CPU_FIRST="4"))
        self.assertEqual((budget.cpu_text(result.build), result.locks), ("0-3", []))
        self.assertIsNone(budget.base_from({"JUNTO_LINUX_MEMORY": "8g"}))

    def test_a_lock_whose_pair_is_in_the_base_sets_is_not_taken(self):
        environment = dict(self.environment, JUNTO_LINUX_CPU_FIRST="4", JUNTO_LINUX_SERVICE_CPU_FIRST="12")
        result = self.decide(environment=environment)
        self.assertEqual(budget.cpu_text(result.build), "0-7")
        self.assertTrue(self.is_free(self.locks[2]) and self.is_free(self.locks[3]))

    def test_the_command_holds_the_locks_and_gets_the_budget_in_its_environment(self):
        # Without the base budget the command runs with its environment unchanged.
        command = [sys.executable, str(HERE / "budget.py"), "--", "sh", "-c",
                   'printf "%s|%s" "${JUNTO_LINUX_CPU_SET-unset}" "$JUNTO_LINUX_MEMORY"']
        environment = {k: v for k, v in os.environ.items() if not k.startswith("JUNTO_")}
        output = subprocess.run(command, env=dict(environment, JUNTO_LINUX_MEMORY="8g"),
                                capture_output=True, text=True, check=True).stdout
        self.assertEqual(output, "unset|8g")
        if not Path("/proc/meminfo").exists():
            self.skipTest("the available memory of a Linux host")
        output = subprocess.run(command, env=dict(environment, **self.environment),
                                capture_output=True, text=True, check=True).stdout
        self.assertIn("Budget: build processors ", output)


def plan(*lines):
    return "\n".join(lines) + "\n"


class Lanes(unittest.TestCase):
    def setUp(self):
        if os.name == "nt":
            self.skipTest("the lanes run shell commands")

    def run_plan(self, text, wide):
        with tempfile.TemporaryDirectory(prefix="junto-lanes-") as directory:
            failures = Path(directory) / "failures"
            failures.touch()

            def decide(base, commands, environment):
                self.commands = commands
                if wide:
                    return budget.Budget(list(range(0, 12)), base.services, 15 * GIB, [], "test")
                return budget.Budget(base.build, base.services, base.memory, [], "test")
            environment = dict(os.environ, **BASE, JUNTO_HEAVY_LOCK=str(Path(directory) / "heavy.lock"))
            started = time.monotonic()
            read, write = os.pipe()
            saved = os.dup(1)
            os.dup2(write, 1)
            os.close(write)
            try:
                status = lanes.run(text, str(failures), environment, directory, decide=decide)
            finally:
                os.dup2(saved, 1)
                os.close(saved)
            output = b""
            while chunk := os.read(read, 65536):
                output += chunk
            os.close(read)
            return status, time.monotonic() - started, output.decode(), failures.read_text()

    def test_lanes_run_at_the_same_time_with_their_part_of_the_budget(self):
        show = 'sh -c \'echo "$JUNTO_LINUX_INSTANCE $JUNTO_LINUX_CPU_SET $JUNTO_LINUX_MEMORY $JUNTO_LINUX_IMAGE_BUILT ${JUNTO_HEAVY_LOCK##*/}"; sleep 2\''
        text = plan("lane app", "step Window tests", show, "lane api", "step API tests", show,
                    "lane other", "step Other tests", show, "step Unit tests", "sh -c 'echo unit'")
        status, seconds, output, failures = self.run_plan(text, wide=True)
        self.assertEqual((status, failures), (0, ""))
        self.assertLess(seconds, 5.5)
        self.assertIn("Lanes: app, api, other, at the same time", output)
        self.assertIn("app 0-3 5120m 1 heavy.lock-app", output)
        self.assertIn("api 4-7 5120m 1 heavy.lock-api", output)
        self.assertIn("other 8-11 5120m 1 heavy.lock-other", output)
        # The output of each step follows the order of the plan.
        positions = [output.index(f"=== {name}  lane ") for name in ("Window tests", "API tests", "Other tests", "Unit tests")]
        self.assertEqual(positions, sorted(positions))
        self.assertRegex(output, r"=== Lanes\napp +\S+Z  \S+Z +\d+m\d\ds  passed  Window tests\n")

    def test_a_lane_without_services_and_with_its_own_memory(self):
        show = 'sh -c \'echo "$JUNTO_LINUX_INSTANCE ${JUNTO_LINUX_SERVICES-all} $JUNTO_LINUX_MEMORY $JUNTO_LINUX_CARGO_JOBS"\''
        text = plan("lane app memory 6g", "step Window tests", show, "lane screens memory 1g without services",
                    "step Screens", show)
        status, _, output, _ = self.run_plan(text, wide=True)
        self.assertEqual(status, 0, output)
        self.assertEqual(self.commands, 1, "only the lane with services counts its containers")
        # 15 GiB less the 7 GiB of the two lanes leaves 8 GiB, an equal part for each.
        self.assertIn("app all 10240m 1", output)
        self.assertIn("screens none 5120m 1", output)
        for wrong in ["lane app memory", "lane app with services", "lane app memory 6x"]:
            with self.subTest(wrong=wrong), self.assertRaises(ValueError):
                lanes.parse(f"{wrong}\nstep x\ntrue\n")

    def test_lanes_whose_memory_the_budget_does_not_hold_run_one_after_the_other(self):
        show = "sh -c 'echo \"$JUNTO_LINUX_INSTANCE ${JUNTO_LINUX_CPU_SET-base} ${JUNTO_LINUX_SERVICES-all}\"'"
        text = plan("lane app memory 10g", "step Window tests", show, "lane screens memory 6g without services",
                    "step Screens", show)
        status, _, output, _ = self.run_plan(text, wide=True)
        self.assertEqual(status, 0, output)
        self.assertIn("Lanes: app, screens, one after the other", output)
        self.assertIn("app base all", output)
        self.assertIn("screens base none", output)

    def test_without_a_budget_for_every_lane_the_lanes_run_one_after_the_other(self):
        show = "sh -c 'echo \"$JUNTO_LINUX_INSTANCE ${JUNTO_LINUX_CPU_SET-base}\"; sleep 1'"
        text = plan("lane app", "step Window tests", show, "lane api", "step API tests", show)
        status, seconds, output, _ = self.run_plan(text, wide=False)
        self.assertEqual(status, 0)
        self.assertGreaterEqual(seconds, 2)
        self.assertIn("Lanes: app, api, one after the other", output)
        self.assertIn("app base", output)

    def test_every_step_runs_and_every_failed_command_is_listed(self):
        text = plan("lane app", "step Window tests", "sh -c 'exit 3'", "sh -c 'echo after'",
                    "lane other", "step Other tests", "sh -c 'exit 5'", "step Unit tests", "true")
        status, _, output, failures = self.run_plan(text, wide=True)
        self.assertEqual(status, 3)
        self.assertIn("after", output)
        self.assertEqual(failures, "Window tests (exit 3): sh -c 'exit 3'\nOther tests (exit 5): sh -c 'exit 5'\n")
        self.assertRegex(output, r"other +\S+Z  \S+Z +\d+m\d\ds  passed  Unit tests\n")

    def test_a_plan_with_an_error_is_refused(self):
        for text in ["step Window tests\ntrue\n", "lane App\nstep x\ntrue\n", "lane app\ntrue\n",
                     "lane a\nstep x\ntrue\nlane a\nstep y\ntrue\n", "lane a\n"]:
            with self.subTest(text=text), self.assertRaises(ValueError):
                lanes.parse(text)

    def test_the_processors_are_split_into_contiguous_groups_by_share(self):
        self.assertEqual(lanes.split(list(range(10)), [1, 1, 1]), [[0, 1, 2, 3], [4, 5, 6], [7, 8, 9]])
        self.assertEqual([len(group) for group in lanes.split(list(range(12)), [3, 2, 1, 1])], [5, 3, 2, 2])
        self.assertEqual(lanes.split([0, 1, 2], [5, 1, 1]), [[0], [1], [2]])
        self.assertEqual(lanes.parse("lane app share 3\nstep x\ntrue\n")[0][0].share, 3)
        for wrong in ["share 0", "share", "share x"]:
            with self.subTest(wrong=wrong), self.assertRaises(ValueError):
                lanes.parse(f"lane app {wrong}\nstep x\ntrue\n")


def artifact(name, kind, test=True):
    return json.dumps({"reason": "compiler-artifact", "profile": {"test": test},
                       "target": {"name": name, "kind": [kind]}})


class CargoTargets(unittest.TestCase):
    def test_every_test_target_of_the_build_but_the_excluded_ones(self):
        lines = [artifact("junto_server", "lib"), artifact("app", "test"), artifact("api", "test"),
                 artifact("e2e", "test"), artifact("rules", "test"), artifact("e2e", "test"),
                 artifact("junto_server", "lib", test=False), '{"reason":"build-finished","success":true}',
                 "not a message"]
        self.assertEqual(cargo_targets.options(lines, ["app", "api"]),
                         ["--lib", "--bins", "--test", "e2e", "--test", "rules"])

    def test_an_excluded_target_that_the_build_does_not_have_is_an_error(self):
        with self.assertRaisesRegex(ValueError, "no test target api"):
            cargo_targets.options([artifact("app", "test"), artifact("e2e", "test")], ["app", "api"])
        with self.assertRaisesRegex(ValueError, "no other test target"):
            cargo_targets.options([artifact("app", "test")], ["app"])


if __name__ == "__main__":
    unittest.main()
