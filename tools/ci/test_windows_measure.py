"""The optional Windows job rust-windows-measure: ci.sh sends it to the Windows runner only,
and run-jobs.sh runs its script with the tools of the Windows job (ADR 0905)."""
import os
import re
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]


def program(path, body):
    path.write_text("#!/bin/sh\n" + body + "\n")
    path.chmod(0o755)


class WindowsMeasureJob(unittest.TestCase):
    def dispatch(self, *jobs):
        """ci.sh run with programs that record the commands for the runners."""
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            program(root / "git", '''case "$*" in
*--git-common-dir*) echo "$FAKE_ROOT/.git" ;;
*rev-parse*) echo 1111111111111111111111111111111111111111 ;;
esac''')
            program(root / "ssh", 'echo "$*" >>"$FAKE_ROOT/ssh.log"\ncase "$*" in *" -s") cat >>"$FAKE_ROOT/ssh.log" ;; esac')
            program(root / "scp", "exit 0")
            environment = dict(os.environ, PATH=f"{root}:{os.environ['PATH']}", FAKE_ROOT=str(root),
                               JUNTO_CI_HOST="linux.example.test", JUNTO_CI_DIR="/ci",
                               JUNTO_CI_WINDOWS_HOST="windows.example.test", JUNTO_CI_WINDOWS_DIR="C:/ci",
                               JUNTO_CI_WINDOWS_USER="Example", JUNTO_CI_WINDOWS_KEY="example-key")
            result = subprocess.run(["sh", str(ROOT / "tools/ci/ci.sh"), "run", "HEAD", *jobs],
                                    env=environment, stdin=subprocess.DEVNULL, capture_output=True, text=True,
                                    timeout=30)
            log = root / "ssh.log"
            return result, log.read_text() if log.exists() else ""

    @staticmethod
    def jobs_of(commands, runner):
        """The jobs that ci.sh wrote for the run of `runner`."""
        return [found.split() for found in re.findall(rf"echo '([^']*)' >'?[^ ]*-{runner}/jobs", commands)
                ] + [found.split() for found in re.findall(r"echo '([^']*)' >jobs", commands)
                     if runner == "linux"]

    def test_the_measurement_job_is_sent_to_the_windows_runner_only(self):
        result, commands = self.dispatch("rust-windows-measure")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.jobs_of(commands, "windows"), [["rust-windows-measure"]])
        self.assertEqual(self.jobs_of(commands, "linux"), [])
        self.assertNotIn("linux.example.test", commands)

    def test_the_default_jobs_do_not_include_the_measurement(self):
        result, commands = self.dispatch()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.jobs_of(commands, "windows"), [["rust-windows"]])
        self.assertNotIn("rust-windows-measure", commands)

    def run_job(self, job):
        """run-jobs.sh in its mode of one job, on a checkout whose two Windows scripts record
        that they ran, with stand-ins for the tools of Git for Windows and rustup."""
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            record = root / "record"
            checkout = root / "ci" / "checkout"
            (checkout / "tools" / "ci").mkdir(parents=True)
            shutil.copy(ROOT / "tools/ci/pins.sh", checkout / "tools/ci")
            (checkout / "rust-toolchain.toml").write_text('[toolchain]\nchannel = "1.98.1"\n')
            program(checkout / "tools/ci/rust-windows-measure.sh", 'echo "measure" >>"$RECORD"')
            program(checkout / "tools/ci/rust-other-systems.sh", 'echo "default" >>"$RECORD"')
            (root / "ci" / "runs" / "run").mkdir(parents=True)
            (root / "ci" / "runs" / "run" / "commit").write_text("1" * 40 + "\n")
            bin_directory = root / "bin"
            bin_directory.mkdir()
            program(bin_directory / "cygpath", '[ "$2" = -W ] && { echo /nonexistent; exit 0; }\necho "$2"')
            program(bin_directory / "git", "exit 0")
            for name in ("rm", "mkdir", "sed", "dirname"):
                os.symlink(shutil.which(name), bin_directory / name)
            (root / "ci" / "cargo" / "bin").mkdir(parents=True)
            program(root / "ci" / "cargo" / "bin" / "rustup", 'echo "rustup $1 $2 $3" >>"$RECORD"')
            environment = dict(os.environ, PATH=f"{bin_directory}:{os.environ['PATH']}", RECORD=str(record),
                               JUNTO_CI_LOCKED="1", JUNTO_CI_JOB=job)
            result = subprocess.run(["sh", str(ROOT / "tools/ci/run-jobs.sh"), str(root / "ci"), "run", "windows",
                                     str(root / "repository"), job],
                                    env=environment, capture_output=True, text=True, timeout=60)
            return result, record.read_text().splitlines() if record.exists() else []

    def test_run_jobs_runs_the_script_of_each_windows_job_with_its_toolchain(self):
        for job, script in (("rust-windows-measure", "measure"), ("rust-windows", "default")):
            with self.subTest(job=job):
                result, steps = self.run_job(job)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertEqual(steps, ["rustup toolchain install 1.98.1", script])


if __name__ == "__main__":
    unittest.main()
