"""The standalone macOS checks build the icon before using it in a test bundle."""
import contextlib
import importlib
import os
import pathlib
import tempfile
import unittest
from unittest.mock import patch

import release_tools


class BundleReached(Exception):
    pass


class CheckIconTests(unittest.TestCase):
    def test_checks_rebuild_a_missing_or_stale_icon_before_assembling(self):
        for name in ["check_update", "check_signing"]:
            for state in ["missing", "stale"]:
                with self.subTest(check=name, icon=state), tempfile.TemporaryDirectory() as temporary:
                    module = importlib.import_module(name)
                    root = pathlib.Path(temporary)
                    source, tool = root / "source.svg", root / "junto-icon"
                    source.write_text("new source")
                    os.utime(source, (200, 200))
                    if state == "stale":
                        tool.write_text("old rendering")
                        os.utime(tool, (100, 100))

                    def build(*arguments, **options):
                        if arguments[:2] == ("cargo", "build") and "junto-icon" in arguments:
                            self.assertEqual(options["cwd"], release_tools.REPOSITORY)
                            tool.write_text("new rendering")
                            os.utime(tool, (300, 300))

                    def assemble(*arguments, **options):
                        self.assertEqual(options["icon_tool"], tool)
                        self.assertTrue(tool.is_file(), "the check did not build its missing icon program")
                        self.assertGreaterEqual(tool.stat().st_mtime, source.stat().st_mtime,
                                                "the check used an icon program older than the source")
                        self.assertEqual(tool.read_text(), "new rendering")
                        raise BundleReached

                    with contextlib.ExitStack() as patches:
                        patches.enter_context(patch("icons.TOOL", tool))
                        patches.enter_context(patch("release_tools.run", side_effect=build))
                        patches.enter_context(patch("release_tools.keys", return_value={}))
                        patches.enter_context(patch("release_tools.build", return_value=(root / "release", root / "recover")))
                        patches.enter_context(patch.object(module, "run", return_value=""))
                        patches.enter_context(patch.object(module, "signals", contextlib.nullcontext))
                        patches.enter_context(patch.object(module, "create", return_value="test certificate"))
                        patches.enter_context(patch.object(module, "cleanup"))
                        patches.enter_context(patch.object(module, "program", return_value=root / "program"))
                        patches.enter_context(patch.object(module, "assemble", side_effect=assemble))
                        with self.assertRaises(BundleReached):
                            module.check()


if __name__ == "__main__":
    unittest.main()
