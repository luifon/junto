"""Release entry point refuses incomplete signing configuration before any build."""
import os
import pathlib
import subprocess
import unittest


class ReleaseTests(unittest.TestCase):
    @unittest.skipIf(os.name == "nt", "POSIX release entry point")
    def test_release_refuses_adhoc_and_missing_update_source(self):
        script = pathlib.Path(__file__).with_name("release.sh")
        environment = os.environ.copy()
        environment["JUNTO_CODESIGN_IDENTITY"] = "-"
        environment.pop("JUNTO_UPDATE_SOURCE", None)
        result = subprocess.run([str(script)], env=environment, capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("certificate", result.stderr)


if __name__ == "__main__":
    unittest.main()
