"""The Windows ZIP, assembled from stand-in programs on any system."""
import json
import pathlib
import tempfile
import unittest
import zipfile

import windows


class WindowsPackageTests(unittest.TestCase):
    def test_the_zip_holds_one_folder_with_the_programs_the_description_and_the_licences(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            client, recover = root / "junto.exe", root / "junto-recover.exe"
            client.write_bytes(b"MZ client")
            recover.write_bytes(b"MZ recover")
            output = windows.assemble(client, recover, "1.2.3", root / "out", epoch=1_700_000_000)
            with zipfile.ZipFile(output / "Junto-1.2.3-windows-x86_64.zip") as archive:
                names = archive.namelist()
                self.assertTrue(all(name.startswith("Junto/") for name in names), names)
                self.assertEqual(archive.read("Junto/junto.exe"), b"MZ client")
                self.assertEqual(json.loads(archive.read("Junto/junto-package.json")),
                                 {"format": 1, "version": "1.2.3", "system": "windows", "package": "directory",
                                  "architecture": "x86_64"})
                self.assertIn("Junto/licences/Junto-LICENSE.txt", names)
                self.assertEqual({entry.date_time for entry in archive.infolist()}, {(2023, 11, 14, 22, 13, 20)})
            self.assertEqual(sorted(path.name for path in (output / "package-directory").iterdir()), ["Junto"])

    def test_a_file_that_is_not_a_windows_program_is_refused(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            client = root / "junto.exe"
            client.write_bytes(b"\x7fELF")
            with self.assertRaisesRegex(ValueError, "not a Windows program"):
                windows.assemble(client, client, "1.2.3", root / "out")
            self.assertFalse((root / "out").exists())


if __name__ == "__main__":
    unittest.main()
