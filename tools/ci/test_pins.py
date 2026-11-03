"""The worker build must use the same dated Debian archive as the other images, and every action
of a workflow must be named by its commit hash."""
import pathlib
import shutil
import subprocess
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
WORKER = pathlib.Path("workers/transcription/docker/Dockerfile")
WORKFLOW = pathlib.Path(".github/workflows/ci.yml")


def check_change(before, after, path=WORKER):
    with tempfile.TemporaryDirectory(prefix="junto-pins-") as temporary:
        root = pathlib.Path(temporary)
        paths = subprocess.check_output(["git", "ls-files", "-z"], cwd=ROOT).split(b"\0")
        for name in paths:
            if not name:
                continue
            relative = pathlib.Path(name.decode())
            if not (ROOT / relative).exists():
                continue
            target = root / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(ROOT / relative, target)
        changed = root / path
        text = changed.read_text()
        if before not in text:
            raise AssertionError(f"{path} does not hold {before!r}")
        changed.write_text(text.replace(before, after, 1))
        return subprocess.run(["sh", "tools/check-pins.sh"], cwd=root,
                              capture_output=True, text=True, timeout=30)


class WorkerPins(unittest.TestCase):
    def test_a_worker_package_install_outside_the_snapshot_is_refused(self):
        result = check_change(
            "FROM ${PYTHON_IMAGE} AS toolchain\n",
            "FROM ${PYTHON_IMAGE} AS toolchain\nRUN apt-get update && apt-get install -y curl\n")
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn(f"FAIL  {WORKER} installs a Debian package", result.stdout)

    def test_worker_package_downgrades_require_the_snapshot_priority(self):
        result = check_change("Pin-Priority: 1001", "Pin-Priority: 999")
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn(f"FAIL  {WORKER} installs a Debian package", result.stdout)


class WorkflowPins(unittest.TestCase):
    def test_an_action_without_a_commit_hash_is_refused(self):
        result = check_change("actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1",
                              "actions/checkout@v7", WORKFLOW)
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("FAIL  a workflow uses an action without a commit hash", result.stdout)

    def test_a_permission_whose_name_ends_in_uses_is_no_action(self):
        result = check_change("    permissions:\n      contents: read\n",
                              "    permissions:\n      contents: read\n      statuses: read\n", WORKFLOW)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("ok    every action of the workflows has a commit hash", result.stdout)


if __name__ == "__main__":
    unittest.main()
