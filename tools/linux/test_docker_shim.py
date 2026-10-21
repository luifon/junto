"""The Docker command of the Linux build image, which reaches only the test LiveKit server."""

import os
from pathlib import Path
import subprocess
import tempfile
import unittest

SHIM = Path(__file__).resolve().parent / "docker"


@unittest.skipIf(os.name == "nt", "POSIX shell script")
class DockerShim(unittest.TestCase):
    def run_shim(self, *arguments):
        with tempfile.TemporaryDirectory(prefix="junto-docker-shim-") as directory:
            root = Path(directory)
            curl = root / "curl"
            curl.write_text('#!/bin/sh\nprintf "%s\\n" "$*" > "$CURL_LOG"\n')
            curl.chmod(0o755)
            environment = dict(
                os.environ,
                PATH=f"{root}:{os.environ['PATH']}",
                CURL_LOG=str(root / "curl.log"),
                JUNTO_TEST_LIVEKIT_CONTAINER="junto-linux-livekit",
            )
            result = subprocess.run(["sh", str(SHIM), *arguments], env=environment,
                                    capture_output=True, text=True)
            logged = (root / "curl.log").read_text() if result.returncode == 0 else ""
            return result, logged

    def test_the_live_tests_can_stop_start_pause_and_unpause_the_server(self):
        for arguments, action in [
            (["stop", "--time", "1", "junto-linux-livekit"], "stop"),
            (["start", "junto-linux-livekit"], "start"),
            (["pause", "junto-linux-livekit"], "pause"),
            (["unpause", "junto-linux-livekit"], "unpause"),
        ]:
            with self.subTest(action=action):
                result, logged = self.run_shim(*arguments)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertIn(f"-X POST http://control:8080/{action}", logged)

    def test_other_commands_and_other_containers_are_refused(self):
        for arguments in [["rm", "-f", "junto-linux-livekit"], ["pause", "another"],
                          ["run", "alpine"]]:
            with self.subTest(arguments=arguments):
                result, _ = self.run_shim(*arguments)
                self.assertEqual(result.returncode, 2)


if __name__ == "__main__":
    unittest.main()
