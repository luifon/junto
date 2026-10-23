#!/usr/bin/env python3
"""Tests of the Windows runner of tools/ci/ci.sh. Programs named ssh, scp and powershell.exe in
the PATH stand for the Windows machine: ssh records its arguments and runs the scripts that
ci.sh sends to the Bash of the machine with sh in a folder of the test, scp copies into that
folder, and powershell.exe answers the question whether a run has ended."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

CI = Path(__file__).with_name("ci.sh")
ROOT = Path(__file__).resolve().parents[2]
SSH = r'''#!/bin/sh
for argument; do printf '[%s]' "$argument"; done >>"$FAKE_ROOT/ssh.log"
echo >>"$FAKE_ROOT/ssh.log"
eval "command=\${$#}"
case "$command" in
    *"bash.exe -s") cd "$FAKE_ROOT/machine" && exec sh -s ;;
    powershell.exe*start-run.ps1*) exit "${FAKE_START_STATUS:-0}" ;;
esac
# The Linux host runs the command in its CI directory, which the command names.
case " $* " in *" linux.example.test "*) exec sh -c "$command" ;; esac
'''
SCP = r'''#!/bin/sh
for argument; do printf '[%s]' "$argument"; done >>"$FAKE_ROOT/scp.log"
echo >>"$FAKE_ROOT/scp.log"
eval "target=\${$#}"
while [ "$#" -gt 1 ]; do
    case "$1" in -o | -i) shift ;; -*) ;; *) cp "$1" "$FAKE_ROOT/machine/${target#*:}" || exit 1 ;; esac
    shift
done
exit "${FAKE_SCP_STATUS:-0}"
'''
# FAKE_FINISH_DURING_CHECK: the run writes its record and "finished" while -Ended runs, and
# its process then ends.
POWERSHELL = r'''#!/bin/sh
case " $* " in
    *" -Ended "*)
        if [ "${FAKE_REQUIRE_START_SCRIPT:-}" = 1 ]; then
            case " $* " in
                *" -File check-run.ps1 "*) test -f check-run.ps1 || exit 2 ;;
                *) test -f start-run.ps1 || exit 2 ;;
            esac
        fi
        if [ -n "${FAKE_FINISH_DURING_CHECK:-}" ]; then
            printf '%s\trust-windows\tstart\tend\t0\tlogs/rust-windows.log\t61\n' "$(cat commit)" >>record.tsv
            rm -f current
            echo 0 >finished
        fi
        exit "${FAKE_ENDED:-1}" ;;
esac
exit 1
'''


class WindowsRunner(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="junto-ci-windows-")
        self.root = Path(self.temporary.name)
        programs = self.root / "bin"
        programs.mkdir()
        for name, text in (("ssh", SSH), ("scp", SCP), ("powershell.exe", POWERSHELL)):
            (programs / name).write_text(text)
            (programs / name).chmod(0o755)
        (self.root / "machine").mkdir()
        (self.root / "tmp").mkdir()
        self.environment = dict(
            os.environ, PATH=f"{programs}:{os.environ['PATH']}", FAKE_ROOT=str(self.root),
            TMPDIR=str(self.root / "tmp"), JUNTO_CI_WINDOWS_HOST="windows.example.test",
            JUNTO_CI_WINDOWS_DIR="C:/ci", JUNTO_CI_WINDOWS_USER="Example User",
            JUNTO_CI_WINDOWS_KEY=str(self.root / "key"))

    def tearDown(self):
        self.temporary.cleanup()

    def ci(self, *arguments, **variables):
        return subprocess.run(["sh", str(CI), *arguments], env=dict(self.environment, **variables),
                              capture_output=True, text=True, timeout=300)

    def log(self, name):
        path = self.root / f"{name}.log"
        return path.read_text() if path.exists() else ""

    def test_a_run_sends_the_commit_and_starts_it_through_start_run(self):
        result = self.ci("run", "HEAD", "rust-windows")
        self.assertEqual(result.returncode, 0, result.stderr)
        run = result.stdout.strip()
        self.assertRegex(run, r"^\d{8}T\d{6}Z-[0-9a-f]{12}-windows$")
        here = f"C:/ci/runs/{run}"
        options = "[-o][BatchMode=yes][-o][IdentitiesOnly=yes][-i][{}][-o][User=\"Example User\"]".format(
            self.root / "key")
        self.assertIn(f"{options}[windows.example.test][C:/ci/tools/git/bin/bash.exe -s]", self.log("ssh"))
        self.assertIn(f"{options}[windows.example.test][powershell.exe -NoProfile -NonInteractive "
                      f"-ExecutionPolicy Bypass -File {here}/start-run.ps1 -Directory C:/ci -Run {run}]",
                      self.log("ssh"))
        copied = self.root / "machine" / here
        self.assertEqual(sorted(path.name for path in copied.iterdir()),
                         ["commit", "commit.bundle", "jobs", "run-jobs.sh", "start-run.ps1", "start.sh"])
        self.assertEqual((copied / "start.sh").read_text(),
                         f"cd '{here}' && : >started && exec sh run-jobs.sh 'C:/ci' '{run}' windows "
                         f"'{here}/commit.bundle' rust-windows </dev/null >runner.log 2>&1\n")
        self.assertEqual(list((self.root / "tmp").iterdir()), [], "the temporary folder of the bundle stays")

    def test_a_commit_without_the_windows_runner_is_refused_and_leaves_no_files(self):
        added = subprocess.run(["git", "-C", str(ROOT), "log", "--diff-filter=A", "--format=%H", "--",
                                "tools/windows/start-run.ps1"], capture_output=True, text=True).stdout.split()
        before = subprocess.run(["git", "-C", str(ROOT), "rev-parse", "--verify", "--quiet", f"{added[-1]}^"],
                                capture_output=True, text=True).stdout.strip() if added else ""
        if not before:
            self.skipTest("the history before the Windows runner is not in this clone")
        result = self.ci("run", before, "rust-windows")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("has no Windows runner", result.stderr)
        self.assertNotIn("fatal:", result.stderr)
        self.assertEqual(list((self.root / "tmp").iterdir()), [], "the temporary folder of the bundle stays")
        self.assertNotIn("start-run.ps1 -Directory", self.log("ssh"))

    def run_folder(self, run):
        here = self.root / "machine" / "C:" / "ci" / "runs" / run
        here.mkdir(parents=True)
        (here / "commit").write_text("0" * 40 + "\n")
        (here / "jobs").write_text("rust-windows\n")
        (here / "current").write_text("rust-windows\t2026-10-17T04:43:29Z\n")
        (here / "process").write_text("4242 134000000000000000\n")
        return here

    # A logoff or a restart of Windows ends the processes of a run before they write "finished".
    def test_a_run_whose_processes_have_ended_without_a_result_is_failed_and_can_be_forgotten(self):
        run = "20261017T044315Z-db9034e4e0e6-windows"
        here = self.run_folder(run)
        running = self.ci("status", run, FAKE_ENDED="1")
        self.assertEqual(running.returncode, 3, running.stdout + running.stderr)
        self.assertIn("running  since 2026-10-17T04:43:29Z", running.stdout)
        refused = self.ci("forget", run, FAKE_ENDED="1")
        self.assertNotEqual(refused.returncode, 0)
        self.assertIn("the run has not finished", refused.stderr)
        self.assertTrue(here.exists())

        ended = self.ci("status", run, FAKE_ENDED="0")
        self.assertEqual(ended.returncode, 1, ended.stdout + ended.stderr)
        self.assertIn("ENDED    without a result", ended.stdout)
        self.assertIn("ended without its result", ended.stdout)
        forgotten = self.ci("forget", run, FAKE_ENDED="0")
        self.assertEqual(forgotten.returncode, 0, forgotten.stdout + forgotten.stderr)
        self.assertFalse(here.exists())

    # The console host of the run was not found, or the start ended before it wrote "process".
    def test_a_run_without_a_process_file_is_asked_whether_it_has_ended(self):
        run = "20261017T044315Z-db9034e4e0e6-windows"
        here = self.run_folder(run)
        (here / "process").unlink()
        self.assertEqual(self.ci("status", run, FAKE_ENDED="1").returncode, 3)
        ended = self.ci("status", run, FAKE_ENDED="0")
        self.assertEqual(ended.returncode, 1, ended.stdout + ended.stderr)
        self.assertIn("ended without its result", ended.stdout)
        self.assertEqual(self.ci("forget", run, FAKE_ENDED="0").returncode, 0)
        self.assertFalse(here.exists())

    def test_a_run_without_its_inspector_is_forgotten_only_after_a_positive_end_check(self):
        run = "20261017T044315Z-db9034e4e0e6-windows"
        here = self.run_folder(run)
        self.assertFalse((here / "start-run.ps1").exists())
        refused = self.ci("forget", run, FAKE_ENDED="1", FAKE_REQUIRE_START_SCRIPT="1")
        self.assertNotEqual(refused.returncode, 0)
        self.assertTrue(here.exists())
        forgotten = self.ci("forget", run, FAKE_ENDED="0", FAKE_REQUIRE_START_SCRIPT="1")
        self.assertEqual(forgotten.returncode, 0, forgotten.stdout + forgotten.stderr)
        self.assertFalse(here.exists())

    def test_a_run_without_start_evidence_is_not_forgotten_during_its_upload(self):
        run = "20261017T044315Z-db9034e4e0e6-windows"
        here = self.run_folder(run)
        (here / "process").unlink()
        (here / "current").unlink()
        result = self.ci("forget", run, FAKE_ENDED="0")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("upload may still be in progress", result.stderr)
        self.assertTrue(here.exists())
        self.assertEqual(self.log("scp"), "")

    # The run passes while status asks whether its processes have ended.
    def test_a_run_that_finishes_during_the_check_of_its_end_is_reported_with_its_result(self):
        run = "20261017T044315Z-db9034e4e0e6-windows"
        here = self.run_folder(run)
        result = self.ci("status", run, FAKE_ENDED="0", FAKE_FINISH_DURING_CHECK="1")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("rust-windows           passed   1m1s", result.stdout)
        self.assertNotIn("ENDED", result.stdout)
        self.assertTrue(here.exists())

    # A copy that fails leaves a run that status reports as failed and forget removes.
    def test_a_failed_copy_records_the_end_of_the_run_and_removes_the_bundle(self):
        result = self.ci("run", "HEAD", "rust-windows", FAKE_SCP_STATUS="1")
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertNotIn("start-run.ps1 -Directory", self.log("ssh"))
        runs = list((self.root / "machine" / "C:" / "ci" / "runs").iterdir())
        self.assertEqual(len(runs), 1)
        self.assertEqual((runs[0] / "finished").read_text(), "1\n")
        # The jobs of the run are recorded before the copy, so status does not take an earlier run.
        self.assertEqual((runs[0] / "jobs").read_text(), "rust-windows\n")
        self.assertFalse((runs[0] / "commit.bundle").exists())
        self.assertEqual(self.ci("status", runs[0].name).returncode, 1)
        self.assertEqual(self.ci("forget", runs[0].name).returncode, 0)
        self.assertFalse(runs[0].exists())

    # After a lost connection the run can have started: its end is not recorded.
    def test_a_start_whose_connection_ends_leaves_the_run_to_status(self):
        result = self.ci("run", "HEAD", "rust-windows", FAKE_START_STATUS="255")
        self.assertEqual(result.returncode, 255, result.stdout + result.stderr)
        self.assertIn("ended with status 255; ci.sh status", result.stderr)
        runs = list((self.root / "machine" / "C:" / "ci" / "runs").iterdir())
        self.assertFalse((runs[0] / "finished").exists())
        self.assertEqual(list((self.root / "tmp").iterdir()), [], "the temporary folder of the bundle stays")

    def test_a_run_that_did_not_start_is_failed_and_can_be_forgotten(self):
        run = "20261017T044315Z-db9034e4e0e6-windows"
        here = self.run_folder(run)
        for name in ("current", "process", "jobs"):
            (here / name).unlink()
        (here / "runner.log").write_text("the run cannot start: the screen is locked\n")
        (here / "finished").write_text("2\n")
        result = self.ci("status", run)
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("the runner ended with status 2", result.stdout)
        self.assertEqual(self.ci("forget", run).returncode, 0)
        self.assertFalse(here.exists())

    # tools/ci/caches.sh refused the run before its first job (ADR 0905).
    def test_a_run_that_ended_before_its_first_job_shows_the_reason(self):
        run = "20261017T044315Z-db9034e4e0e6-windows"
        here = self.run_folder(run)
        for name in ("current", "process"):
            (here / name).unlink()
        (here / "runner.log").write_text("the run starts no job: the disk of the caches has 1 KiB free\n")
        (here / "finished").write_text("1\n")
        result = self.ci("status", run)
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("rust-windows           not run", result.stdout)
        self.assertIn("the runner ended with status 1 before its first job", result.stdout)
        self.assertIn("    the run starts no job: the disk of the caches has 1 KiB free", result.stdout)


DEFAULT_JOBS = {
    "linux": ["rust-linux", "licences", "transcription-worker", "layout-page"],
    "macos": ["rust-macos", "deployment"],
    "windows": ["rust-windows"],
}


class CommitStatus(unittest.TestCase):
    """ci.sh status of a commit, with runs of the three runners in folders of the test."""

    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="junto-ci-status-")
        self.root = Path(self.temporary.name)
        programs = self.root / "bin"
        programs.mkdir()
        for name, text in (("ssh", SSH), ("powershell.exe", POWERSHELL)):
            (programs / name).write_text(text)
            (programs / name).chmod(0o755)
        (self.root / "machine").mkdir()
        self.commit = subprocess.run(["git", "-C", str(ROOT), "rev-parse", "HEAD"], capture_output=True,
                                     text=True, check=True).stdout.strip()
        self.environment = dict(
            os.environ, PATH=f"{programs}:{os.environ['PATH']}", FAKE_ROOT=str(self.root),
            JUNTO_CI_HOST="linux.example.test", JUNTO_CI_DIR=str(self.root / "linux"),
            JUNTO_CI_WINDOWS_HOST="windows.example.test", JUNTO_CI_WINDOWS_DIR="C:/ci",
            JUNTO_CI_LOCAL_DIR=str(self.root / "macos"))
        self.directories = {"linux": self.root / "linux", "macos": self.root / "macos",
                            "windows": self.root / "machine" / "C:" / "ci"}
        self.runs = []

    def tearDown(self):
        self.temporary.cleanup()
        # status copies the log of a failed job into target/ci-logs/<run> of the repository.
        for run in self.runs:
            shutil.rmtree(ROOT / "target" / "ci-logs" / run, ignore_errors=True)

    def run_of(self, runner, started, jobs, results):
        """A finished run of the commit: `results` maps each job that ran to its exit status."""
        run = f"{started}-{self.commit[:12]}-{runner}"
        self.runs.append(run)
        here = self.directories[runner] / "runs" / run
        here.mkdir(parents=True)
        (here / "commit").write_text(self.commit + "\n")
        (here / "jobs").write_text(" ".join(jobs) + "\n")
        (here / "record.tsv").write_text("".join(
            f"{self.commit}\t{job}\tstart\tend\t{status}\tlogs/{job}.log\t61\n" for job, status in results.items()))
        (here / "logs").mkdir()
        for job in results:
            (here / "logs" / f"{job}.log").write_text(f"the log of {job}\n")
        (here / "finished").write_text(("1" if any(results.values()) else "0") + "\n")
        return run

    def status(self):
        return subprocess.run(["sh", str(CI), "status", self.commit], env=self.environment, capture_output=True,
                              text=True, timeout=300, cwd=self.root)

    def passed_elsewhere(self):
        self.run_of("macos", "20261020T100000Z", ["rust-macos", "deployment"], {"rust-macos": 0, "deployment": 0})
        self.run_of("windows", "20261020T100000Z", ["rust-windows"], {"rust-windows": 0})

    # rust-linux failed once and passed in a run of that job alone; the other jobs passed in the
    # first run.
    def test_a_run_of_the_failed_job_alone_completes_the_earlier_run(self):
        self.passed_elsewhere()
        linux = ["rust-linux", "licences", "transcription-worker", "layout-page"]
        self.run_of("linux", "20261020T100000Z", linux, {"rust-linux": 1, "licences": 0,
                                                          "transcription-worker": 0, "layout-page": 0})
        later = self.run_of("linux", "20261020T110000Z", ["rust-linux"], {"rust-linux": 0})
        result = self.status()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn(f"commit {self.commit}: passed", result.stdout)
        self.assertIn(f"run {later}", result.stdout)
        self.assertNotIn("FAILED", result.stdout)

    # The latest run of a job decides it, also when an earlier run passed it.
    def test_a_job_that_failed_in_its_latest_run_fails_the_commit(self):
        self.passed_elsewhere()
        linux = ["rust-linux", "licences", "transcription-worker", "layout-page"]
        self.run_of("linux", "20261020T100000Z", linux, dict.fromkeys(linux, 0))
        self.run_of("linux", "20261020T110000Z", ["licences"], {"licences": 1})
        result = self.status()
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn(f"commit {self.commit}: failed", result.stdout)
        self.assertRegex(result.stdout, r"licences +FAILED")

    # A later start that failed before its runner wrote the jobs: the earlier pass does not decide them.
    def test_a_later_run_without_its_jobs_is_not_passed(self):
        self.run_of("linux", "20261020T100000Z", ["rust-linux", "licences", "transcription-worker", "layout-page"],
                    dict.fromkeys(["rust-linux", "licences", "transcription-worker", "layout-page"], 0))
        self.passed_elsewhere()
        failed = self.run_of("windows", "20261020T110000Z", ["rust-windows"], {})
        here = self.directories["windows"] / "runs" / failed
        for name in ("jobs", "record.tsv"):
            (here / name).unlink()
        (here / "finished").write_text("2\n")
        (here / "runner.log").write_text("the run cannot start: the screen is locked\n")
        result = self.status()
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn(f"run {failed}", result.stdout)
        self.assertIn(f"commit {self.commit}: failed", result.stdout)

    # A failure of one runner stays the verdict when a later runner lacks a job.
    def test_a_failed_job_stays_failed_when_another_job_has_no_run(self):
        linux = ["rust-linux", "licences", "transcription-worker", "layout-page"]
        self.run_of("linux", "20261020T100000Z", linux, {"rust-linux": 0, "licences": 1,
                                                          "transcription-worker": 0, "layout-page": 0})
        self.run_of("macos", "20261020T100000Z", ["rust-macos"], {"rust-macos": 0})
        self.run_of("windows", "20261020T100000Z", ["rust-windows"], {"rust-windows": 0})
        result = self.status()
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn(f"macos: no run of {self.commit} has deployment", result.stdout)
        self.assertIn(f"commit {self.commit}: failed", result.stdout)

    def test_a_job_without_a_run_leaves_the_commit_incomplete(self):
        self.passed_elsewhere()
        self.run_of("linux", "20261020T100000Z", ["rust-linux", "licences", "layout-page"],
                    {"rust-linux": 0, "licences": 0, "layout-page": 0})
        result = self.status()
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn(f"linux: no run of {self.commit} has transcription-worker", result.stdout)
        self.assertIn(f"commit {self.commit}: incomplete", result.stdout)

    def passing(self, runner, started="20261020T100000Z", **extra):
        """A run of the default jobs of a runner that each passed, with the extra jobs and statuses."""
        results = {**dict.fromkeys(DEFAULT_JOBS[runner], 0), **extra}
        return self.run_of(runner, started, list(results), results)

    def test_a_failed_optional_job_is_shown_and_does_not_fail_the_commit(self):
        self.passing("linux", **{"rust-linux-devices": 101})
        self.passed_elsewhere()
        result = self.status()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertRegex(result.stdout, r"rust-linux-devices +FAILED .*  \(optional\)")
        # run-jobs.sh ends with 1 because of the optional job; that is not a failure outside the jobs.
        self.assertNotIn("the runner ended with status", result.stdout)
        self.assertIn(f"commit {self.commit}: passed", result.stdout)

    # The run of the optional job alone is shown and decides no default job.
    def test_a_later_run_of_only_the_optional_job_leaves_the_full_run_to_decide(self):
        full = self.passing("linux")
        later = self.run_of("linux", "20261020T110000Z", ["rust-linux-devices"], {"rust-linux-devices": 0})
        self.passed_elsewhere()
        result = self.status()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn(f"run {full}", result.stdout)
        self.assertIn(f"run {later}", result.stdout)
        self.assertIn(f"commit {self.commit}: passed", result.stdout)

    def test_a_later_failed_run_of_only_the_optional_job_does_not_fail_the_commit(self):
        self.passing("linux")
        self.run_of("linux", "20261020T110000Z", ["rust-linux-devices"], {"rust-linux-devices": 101})
        self.passed_elsewhere()
        result = self.status()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertRegex(result.stdout, r"rust-linux-devices +FAILED .*  \(optional\)")
        self.assertIn(f"commit {self.commit}: passed", result.stdout)

    # A run of only the optional job that ended before its first job.
    def test_a_later_run_of_only_the_optional_job_that_did_not_start_does_not_fail_the_commit(self):
        self.passing("linux")
        failed = self.run_of("linux", "20261020T110000Z", ["rust-linux-devices"], {})
        here = self.directories["linux"] / "runs" / failed
        (here / "record.tsv").unlink()
        (here / "finished").write_text("1\n")
        (here / "runner.log").write_text("the run starts no job: the disk of the caches has 1 KiB free\n")
        self.passed_elsewhere()
        result = self.status()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("rust-linux-devices     not run  (optional)", result.stdout)
        self.assertIn(f"commit {self.commit}: passed", result.stdout)

    # The optional job runs after the default jobs; the run is not finished.
    def test_a_run_whose_optional_job_still_runs_leaves_the_commit_incomplete(self):
        run = self.passing("linux")
        here = self.directories["linux"] / "runs" / run
        (here / "jobs").write_text(" ".join(DEFAULT_JOBS["linux"] + ["rust-linux-devices"]) + "\n")
        (here / "finished").unlink()
        (here / "current").write_text("rust-linux-devices\t2026-10-20T10:30:00Z\n")
        self.passed_elsewhere()
        result = self.status()
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("rust-linux-devices     running  since 2026-10-20T10:30:00Z  (optional)", result.stdout)
        self.assertIn(f"commit {self.commit}: incomplete", result.stdout)

    # ci.sh status <run> decides with every job of the run, the optional job as well.
    def test_the_status_of_a_run_fails_with_its_optional_job(self):
        run = self.passing("linux", **{"rust-linux-devices": 101})
        result = subprocess.run(["sh", str(CI), "status", run], env=self.environment, capture_output=True,
                                text=True, timeout=300, cwd=self.root)
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertRegex(result.stdout, r"rust-linux-devices +FAILED ")
        self.assertNotIn("(optional)", result.stdout)

    def test_a_failed_default_job_fails_the_commit(self):
        self.passing("linux", **{"rust-linux": 101, "rust-linux-devices": 0})
        self.passed_elsewhere()
        result = self.status()
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn(f"commit {self.commit}: failed", result.stdout)

    def test_a_later_failed_run_of_a_default_job_fails_the_commit(self):
        self.passing("linux")
        self.run_of("linux", "20261020T110000Z", ["rust-linux"], {"rust-linux": 101})
        self.passed_elsewhere()
        result = self.status()
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn(f"commit {self.commit}: failed", result.stdout)

    def test_a_commit_with_only_a_run_of_the_optional_job_is_incomplete(self):
        self.run_of("linux", "20261020T110000Z", ["rust-linux-devices"], {"rust-linux-devices": 0})
        self.passed_elsewhere()
        result = self.status()
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        for job in DEFAULT_JOBS["linux"]:
            self.assertIn(f"linux: no run of {self.commit} has {job}", result.stdout)
        self.assertIn(f"commit {self.commit}: incomplete", result.stdout)


if __name__ == "__main__":
    unittest.main()
