#!/usr/bin/env python3
"""Check privilege dropping and ownership migration on Linux."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

LAUNCHER = Path(__file__).with_name("run-as-user.sh")


class UserLauncher(unittest.TestCase):
    @unittest.skipUnless(sys.platform == "linux" and os.getuid() == 0, "needs a root Linux container")
    def test_camera_checks_keep_only_the_explicit_device_groups(self):
        result = subprocess.run([
            LAUNCHER, "1000", "1000", "--", sys.executable, "-c",
            "import os; assert os.getuid()==1000 and os.getgid()==1000; assert os.getgroups()==[44,45]",
        ], env=dict(os.environ, JUNTO_TEST_CAMERA_DEVICE="1", JUNTO_LINUX_CAMERA_GROUPS="44,45"),
            capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_root_is_rejected(self):
        result = subprocess.run([LAUNCHER, "0", "0", "--", "true"], capture_output=True)
        self.assertEqual(result.returncode, 2)

    @unittest.skipUnless(sys.platform == "linux" and os.getuid() == 0, "needs a root Linux container")
    def test_existing_mixed_ownership_is_migrated_and_permissions_apply(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            os.chown(directory, 1000, 1000)
            readonly = directory / "readonly"
            readonly.mkdir()
            readonly.chmod(0o555)
            (directory / "old-cache-entry").write_text("cached")
            result = subprocess.run([
                LAUNCHER, "1000", "1000", directory, "--", sys.executable, "-c",
                "import os,pathlib,sys; p=pathlib.Path(sys.argv[1]); "
                "assert os.getuid()==1000 and os.getgid()==1000 and os.getgroups()==[]; "
                "assert (p/'old-cache-entry').stat().st_uid==1000; "
                "assert not os.access(p/'readonly',os.W_OK); "
                "(p/'created').write_text('normal user')", directory,
            ], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual((directory / "created").stat().st_uid, 1000)
            self.assertEqual((readonly.stat().st_uid, readonly.stat().st_gid), (1000, 1000))

    @unittest.skipUnless(sys.platform == "linux" and os.getuid() == 0, "needs a root Linux container")
    def test_the_command_failure_is_preserved(self):
        result = subprocess.run([LAUNCHER, "1000", "1000", "--", "sh", "-c", "exit 37"])
        self.assertEqual(result.returncode, 37)


if __name__ == "__main__":
    unittest.main()
