"""The Linux restart driver waits for detached helpers and removes them on failure."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

import check_lifecycle


@unittest.skipUnless(sys.platform.startswith("linux"), "Linux child adoption")
class RestartProcesses(unittest.TestCase):
    def test_a_detached_restart_ends_or_is_reaped_after_timeout_or_interruption(self):
        for mode in ("finish", "timeout", "interrupt"):
            with self.subTest(mode=mode), tempfile.TemporaryDirectory() as temporary:
                record = Path(temporary) / "child"
                child = ("import os,pathlib,signal,sys,time\n"
                         f"pathlib.Path({str(record)!r}).write_text(str(os.getpid()))\n"
                         "deadline=time.monotonic()+5\n"
                         "while pathlib.Path('/proc/'+sys.argv[1]).exists() and time.monotonic()<deadline: time.sleep(.01)\n"
                         "assert not pathlib.Path('/proc/'+sys.argv[1]).exists()\n"
                         + ("print('restart finished',flush=True)" if mode == "finish" else "signal.pause()"))
                parent = f"import os,subprocess,sys; subprocess.Popen([sys.executable,'-c',{child!r},str(os.getpid())],start_new_session=True)"
                bound = 20 if mode == "interrupt" else 3
                driver = (f"import sys; sys.path.insert(0,{str(Path(__file__).parent)!r}); "
                          f"import check_restart; check_restart.check([sys.executable,'-c',{parent!r}],timeout={bound})")
                if mode == "interrupt":
                    with self.assertRaisesRegex(AssertionError, "timed out"):
                        check_lifecycle.command([sys.executable, "-c", driver], timeout=3, graceful=True)
                else:
                    result = subprocess.run([sys.executable, "-c", driver], capture_output=True, text=True, timeout=15)
                    if mode == "finish":
                        self.assertEqual(result.returncode, 0, result.stderr)
                        self.assertIn("restart finished", result.stdout)
                    else:
                        self.assertNotEqual(result.returncode, 0)
                        self.assertIn("did not end within its deadline", result.stderr)
                self.assertTrue(record.exists(), "the detached helper never started")
                self.assertFalse(Path(f"/proc/{record.read_text()}").exists(), "the restart helper was left behind")


if __name__ == "__main__":
    unittest.main()
