"""The optional native job runs only its named steps and propagates failures."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]


class DeviceSteps(unittest.TestCase):
    def run_job(self, fail=""):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "tools/ci").mkdir(parents=True)
            shutil.copy(ROOT / "tools/ci/rust-linux-devices.sh", root / "tools/ci")
            shutil.copy(ROOT / "tools/ci/pins.sh", root / "tools/ci")
            recorder = root / "tools/linux-container.sh"
            recorder.write_text('''#!/bin/sh
echo "camera=${JUNTO_LINUX_CAMERA:-0} $*" >>"$RECORD"
case "$*" in *"$FAIL_ON"*) [ -z "$FAIL_ON" ] || exit 17 ;; esac
''')
            recorder.chmod(0o755)
            record = root / "record"
            result = subprocess.run(["sh", str(root / "tools/ci/rust-linux-devices.sh")],
                                    env=dict(os.environ, RECORD=str(record), FAIL_ON=fail),
                                    capture_output=True, text=True, timeout=30)
            return result.returncode, record.read_text().splitlines()

    def test_steps_keep_hardware_out_of_the_synthetic_live_tiers(self):
        status, steps = self.run_job()
        self.assertEqual(status, 0)
        self.assertEqual(steps, [
            "camera=0 cargo clippy --locked -p junto-media --lib -- -D warnings",
            "camera=0 cargo clippy --locked -p junto-media --all-targets -- -D warnings",
            "camera=0 cargo build --locked --workspace",
            "camera=0 cargo test --locked -p junto-media --lib --test camera_factory",
            "camera=0 cargo test --locked -p junto-media --no-fail-fast -- --ignored",
            "camera=0 cargo test --locked -p junto-client --test app live:: -- --ignored",
            "camera=1 cargo run --locked -p junto-media --features native-device-tests --example camera-device",
        ])

    def test_failed_media_check_does_not_start_a_camera(self):
        status, steps = self.run_job("--no-fail-fast")
        self.assertEqual(status, 17)
        self.assertFalse(any(step.startswith("camera=1") for step in steps))

    def dispatch(self, *jobs):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name, body in {
                "git": '''case "$*" in
*--git-common-dir*) echo "$FAKE_ROOT/.git" ;;
*rev-parse*) echo 1111111111111111111111111111111111111111 ;;
esac''',
                "ssh": 'echo "$*" >>"$FAKE_ROOT/ssh.log"',
                "scp": "exit 0",
            }.items():
                program = root / name
                program.write_text("#!/bin/sh\n" + body + "\n")
                program.chmod(0o755)
            environment = dict(os.environ, PATH=f"{root}:{os.environ['PATH']}", FAKE_ROOT=str(root),
                               JUNTO_CI_HOST="linux.example.test", JUNTO_CI_DIR="/ci",
                               JUNTO_CI_WINDOWS_HOST="windows.example.test", JUNTO_CI_WINDOWS_DIR="C:/ci",
                               JUNTO_CI_WINDOWS_USER="Example", JUNTO_CI_WINDOWS_KEY="example-key")
            result = subprocess.run(["sh", str(ROOT / "tools/ci/ci.sh"), "run", "HEAD", *jobs],
                                    env=environment, capture_output=True, text=True, timeout=30)
            return result, (root / "ssh.log").read_text()

    def test_default_dispatch_does_not_open_devices(self):
        result, commands = self.dispatch()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn("rust-linux-devices", commands)
        self.assertIn("rust-linux licences transcription-worker layout-page", commands)

    def test_device_job_requires_an_explicit_name_and_selects_only_linux(self):
        result, commands = self.dispatch("rust-linux-devices")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("rust-linux-devices", commands)
        self.assertNotIn("windows.example.test", commands)


class LinuxJob(unittest.TestCase):
    def test_the_linux_job_lints_the_device_tests_of_the_media_crate(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "tools/ci").mkdir(parents=True)
            shutil.copy(ROOT / "tools/ci/rust-linux.sh", root / "tools/ci")
            shutil.copy(ROOT / "tools/ci/pins.sh", root / "tools/ci")
            recorder = root / "tools/linux-container.sh"
            recorder.write_text('#!/bin/sh\necho "$*" >>"$RECORD"\n')
            recorder.chmod(0o755)
            record = root / "record"
            result = subprocess.run(["sh", str(root / "tools/ci/rust-linux.sh")],
                                    env=dict(os.environ, RECORD=str(record)),
                                    capture_output=True, text=True, timeout=30)
            self.assertEqual(result.returncode, 0, result.stderr)
            steps = record.read_text().splitlines()
            self.assertIn("cargo clippy --locked -p junto-media --features native-device-tests "
                          "--all-targets -- -D warnings", steps)
            self.assertLess(steps.index("cargo clippy --workspace --all-targets --locked --keep-going -- -D warnings"),
                            steps.index("cargo clippy --locked -p junto-media --features native-device-tests "
                                        "--all-targets -- -D warnings"))


if __name__ == "__main__":
    unittest.main()
