"""The worker build must use the same dated Debian archive as the other images."""
import pathlib
import shutil
import subprocess
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
WORKER = pathlib.Path("workers/transcription/docker/Dockerfile")


class WorkerPins(unittest.TestCase):
    def check_change(self, before, after):
        with tempfile.TemporaryDirectory(prefix="junto-pins-") as temporary:
            root = pathlib.Path(temporary)
            paths = subprocess.check_output(["git", "ls-files", "-z"], cwd=ROOT).split(b"\0")
            for name in paths:
                if not name:
                    continue
                relative = pathlib.Path(name.decode())
                target = root / relative
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(ROOT / relative, target)
            worker = root / WORKER
            worker.write_text(worker.read_text().replace(before, after))
            return subprocess.run(["sh", "tools/check-pins.sh"], cwd=root,
                                  capture_output=True, text=True, timeout=30)

    def test_a_worker_package_install_outside_the_snapshot_is_refused(self):
        result = self.check_change(
            "FROM ${PYTHON_IMAGE} AS toolchain\n",
            "FROM ${PYTHON_IMAGE} AS toolchain\nRUN apt-get update && apt-get install -y curl\n")
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn(f"FAIL  {WORKER} installs a Debian package", result.stdout)

    def test_worker_package_downgrades_require_the_snapshot_priority(self):
        result = self.check_change("Pin-Priority: 1001", "Pin-Priority: 999")
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn(f"FAIL  {WORKER} installs a Debian package", result.stdout)


if __name__ == "__main__":
    unittest.main()
