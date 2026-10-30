"""Removal of this user's matching startup entry by the real Linux client."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


@unittest.skipUnless(sys.platform == "linux" and os.environ.get("JUNTO_TEST_STARTUP_PROGRAM"),
                     "needs the built Linux client")
class StartupRemoval(unittest.TestCase):
    def test_removal_clears_only_this_users_matching_enabled_entry(self):
        program = Path(os.environ["JUNTO_TEST_STARTUP_PROGRAM"]).resolve()
        with tempfile.TemporaryDirectory(prefix="junto-startup-removal-") as temporary:
            root = Path(temporary)
            entry = root / "config/autostart/chat.junto.Junto.desktop"
            entry.parent.mkdir(parents=True)
            content = f'[Desktop Entry]\nType=Application\nName=Junto\nExec="{program}" --no-activate\n'
            entry.write_text(content)
            other = root / "other-user/autostart/chat.junto.Junto.desktop"
            other.parent.mkdir(parents=True)
            other.write_text(content)
            environment = dict(os.environ, XDG_CONFIG_HOME=str(root / "config"),
                               XDG_CONFIG_DIRS=str(root / "system"))
            def remove():
                return subprocess.run([str(program), "--remove-startup-entry"], env=environment,
                                      capture_output=True, text=True, timeout=10)
            result = remove()
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertFalse(entry.exists(), "the matching enabled entry survives removal")
            self.assertEqual(other.read_text(), content)
            self.assertEqual(remove().returncode, 0, "removal is idempotent")
            unrelated = '[Desktop Entry]\nType=Application\nExec="/opt/another/junto"\n'
            entry.write_text(unrelated)
            self.assertEqual(remove().returncode, 0)
            self.assertEqual(entry.read_text(), unrelated)
            entry.unlink()
            entry.symlink_to(other)
            self.assertNotEqual(remove().returncode, 0, "a symlink entry is refused")
            self.assertEqual(other.read_text(), content)
