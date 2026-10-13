#!/usr/bin/env python3
"""Check the participant helper without building a second Cargo profile."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest


class ParticipantProfile(unittest.TestCase):
    def test_the_server_path_uses_the_debug_participant(self):
        helper = Path(__file__).with_name("server-live.sh").resolve()
        with tempfile.TemporaryDirectory(prefix="junto-linux-participant-", dir="/tmp") as directory:
            root = Path(directory)
            target = root / "spikes/test-participant/target"
            cargo = root / "cargo"
            cargo.write_text('''#!/bin/sh
set -eu
if [ "$1" = build ]; then
    for argument in "$@"; do
        if [ "$argument" = --release ]; then
            echo 'a second profile is forbidden' >&2
            exit 73
        fi
    done
    mkdir -p "$CARGO_TARGET_DIR/debug"
    printf participant > "$CARGO_TARGET_DIR/debug/test-participant"
else
    test "$(cat spikes/test-participant/target/release/test-participant)" = participant
    exit 29
fi
''')
            cargo.chmod(0o755)
            result = subprocess.run(
                ["sh", str(helper), "--show-output"], cwd=root,
                env=dict(os.environ, PATH=f"{root}:{os.environ['PATH']}", CARGO_TARGET_DIR=str(target)),
                capture_output=True, text=True,
            )
            self.assertEqual(result.returncode, 29, result.stderr)
            self.assertTrue((target / "release/test-participant").samefile(target / "debug/test-participant"))


if __name__ == "__main__":
    unittest.main()
