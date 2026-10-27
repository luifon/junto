"""The disk image of a macOS release, with the macOS tools replaced (ADR 0728)."""
import os
import pathlib
import tempfile
import unittest

import dmg


class DiskImageTests(unittest.TestCase):
    def bundle(self, root):
        bundle = root / "Junto.app"
        (bundle / "Contents").mkdir(parents=True)
        (bundle / "Contents/Info.plist").write_bytes(b"plist")
        return bundle

    def test_the_image_holds_the_bundle_and_a_link_to_the_applications_folder(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            bundle = self.bundle(root)
            seen = []

            def command(*arguments):
                arguments = [str(argument) for argument in arguments]
                seen.append(arguments[0])
                if arguments[0] == "/usr/bin/ditto":
                    pathlib.Path(arguments[2]).mkdir()
                else:
                    volume = pathlib.Path(arguments[arguments.index("-srcfolder") + 1])
                    self.assertEqual(sorted(path.name for path in volume.iterdir()), ["Applications", "Junto.app"])
                    self.assertEqual(os.readlink(volume / "Applications"), "/Applications")
                    self.assertEqual(arguments[arguments.index("-volname") + 1], "Junto")
                    self.assertEqual(arguments[arguments.index("-format") + 1], "UDZO")
                    pathlib.Path(arguments[-1]).write_bytes(b"image")

            output = dmg.assemble(bundle, root / "Junto-0.1.0-macos-aarch64.dmg", command=command)
            self.assertEqual(output.read_bytes(), b"image")
            self.assertEqual(seen, ["/usr/bin/ditto", "/usr/bin/hdiutil"])
            # The staging folder is removed.
            self.assertEqual(sorted(path.name for path in root.iterdir()),
                             ["Junto-0.1.0-macos-aarch64.dmg", "Junto.app"])

    def test_an_existing_image_another_bundle_and_another_name_are_refused(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            bundle = self.bundle(root)
            (root / "taken.dmg").write_bytes(b"")
            with self.assertRaises(FileExistsError):
                dmg.assemble(bundle, root / "taken.dmg", command=lambda *_: None)
            with self.assertRaises(ValueError):
                dmg.assemble(bundle, root / "Junto.zip", command=lambda *_: None)
            (root / "Other.app/Contents").mkdir(parents=True)
            with self.assertRaises(ValueError):
                dmg.assemble(root / "Other.app", root / "new.dmg", command=lambda *_: None)
            with self.assertRaises(RuntimeError):
                dmg.assemble(bundle, root / "none.dmg", command=lambda *arguments: None)


if __name__ == "__main__":
    unittest.main()
