"""The Windows ZIP, assembled from stand-in programs on any system."""
import json
import pathlib
import shutil
import subprocess
import tempfile
import unittest
import zipfile

import fixtures
import icons
import licences
import windows

# The pictures of the Windows icon (ADR 0755).
SIZES = [16, 24, 32, 48, 64, 256]
TRIPLE = "x86_64-pc-windows-msvc"


def resource_linker():
    """The linker of the LLVM tools for Windows programs, if this machine has it: in the search
    path, or in the directory of an LLVM version of Debian (the Linux build image)."""
    found = shutil.which("lld-link")
    versions = sorted(pathlib.Path("/usr/lib").glob("llvm-*/bin/lld-link"), reverse=True)
    return found or (str(versions[0]) if versions else None)


def program_with_subsystem(magic, value):
    """The headers of a Windows program, up to the subsystem field."""
    header = 0x80
    data = bytearray(header + 4 + 20 + 70)
    data[:2] = b"MZ"
    data[0x3C:0x40] = header.to_bytes(4, "little")
    data[header:header + 4] = b"PE\0\0"
    optional = header + 24
    data[optional:optional + 2] = magic.to_bytes(2, "little")
    data[optional + 68:optional + 70] = value.to_bytes(2, "little")
    return bytes(data)


class WindowsPackageTests(unittest.TestCase):
    def test_the_subsystem_is_read_from_the_optional_header_of_both_formats(self):
        for magic in (0x10B, 0x20B):
            for value in (windows.WINDOWS_SUBSYSTEM, windows.CONSOLE_SUBSYSTEM):
                self.assertEqual(windows.subsystem(program_with_subsystem(magic, value)), value)
        for broken in [b"ZM" + bytes(200), program_with_subsystem(0x107, 2),
                       program_with_subsystem(0x20B, 2).replace(b"PE\0\0", b"NE\0\0")]:
            with self.assertRaises(ValueError):
                windows.subsystem(broken)

    def test_the_zip_holds_one_folder_with_the_programs_the_description_and_the_licences(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            client, recover = root / "junto.exe", root / "junto-recover.exe"
            client.write_bytes(b"MZ client")
            recover.write_bytes(b"MZ recover")
            installer = root / "junto-installer.exe"
            installer.write_bytes(b"MZ installer")
            (root / "junto.pdb").write_bytes(b"symbols")
            output = windows.assemble(client, recover, "1.2.3", root / "out", epoch=1_700_000_000,
                                      build=fixtures.build_directory(root / "build", TRIPLE), installer=installer)
            with zipfile.ZipFile(output / "Junto-1.2.3-windows-x86_64.zip") as archive:
                names = archive.namelist()
                # The uninstall program of the folder is the installer without an archive (ADR 0728).
                self.assertEqual(archive.read("Junto/uninstall.exe"), b"MZ installer")
                self.assertTrue(all(name.startswith("Junto/") for name in names), names)
                self.assertEqual(archive.read("Junto/junto.exe"), b"MZ client")
                self.assertEqual(json.loads(archive.read("Junto/junto-package.json")),
                                 {"format": 1, "version": "1.2.3", "system": "windows", "package": "directory",
                                  "architecture": "x86_64"})
                self.assertEqual(sorted(name for name in names if name.startswith("Junto/licences/")),
                                 ["Junto/licences/", "Junto/licences/Junto-LICENSE.txt",
                                  f"Junto/licences/{licences.NAME}"])
                self.assertEqual(archive.read("Junto/licences/Junto-LICENSE.txt"),
                                 (licences.ROOT / "LICENSE").read_bytes())
                text = archive.read(f"Junto/licences/{licences.NAME}").decode()
                self.assertTrue(text.startswith(f"Junto 1.2.3 for {TRIPLE}: third-party licences\n"))
                self.assertEqual(licences.listed(text), fixtures.client_graph(TRIPLE))
                self.assertIn(fixtures.WEBRTC_LICENCE.decode(), text)
                self.assertEqual({entry.date_time for entry in archive.infolist()}, {(2023, 11, 14, 22, 13, 20)})
            self.assertEqual(sorted(path.name for path in (output / "package-directory").iterdir()), ["Junto"])

    def test_the_symbols_of_the_program_are_a_file_beside_the_package_and_not_in_it(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            client, recover = root / "junto.exe", root / "junto-recover.exe"
            client.write_bytes(b"MZ client")
            recover.write_bytes(b"MZ recover")
            (root / "junto.pdb").write_bytes(b"symbols")
            output = windows.assemble(client, recover, "1.2.3", root / "out", epoch=1_700_000_000,
                                      build=fixtures.build_directory(root / "build", TRIPLE), installer=recover)
            with zipfile.ZipFile(output / "Junto-1.2.3-windows-x86_64.zip") as archive:
                self.assertFalse(any(name.endswith(".pdb") for name in archive.namelist()))
            self.assertEqual((output / "Junto-1.2.3-windows-x86_64.pdb").read_bytes(), b"symbols")
            self.assertFalse((output / "package-directory/Junto/junto.pdb").exists())

    def test_a_build_without_symbols_is_refused(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            client, recover = root / "junto.exe", root / "junto-recover.exe"
            client.write_bytes(b"MZ client")
            recover.write_bytes(b"MZ recover")
            with self.assertRaisesRegex(ValueError, "no symbols"):
                windows.assemble(client, recover, "1.2.3", root / "out",
                                 build=fixtures.build_directory(root / "build", TRIPLE), installer=recover)

    def test_the_release_tool_writes_the_setup_program_from_the_installer_and_the_folder(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            client, recover, installer = root / "junto.exe", root / "junto-recover.exe", root / "junto-installer.exe"
            for program in [client, recover, installer]:
                program.write_bytes(b"MZ " + program.name.encode())
            (root / "junto.pdb").write_bytes(b"symbols")
            # A stand-in release tool that records its arguments and writes the output.
            tool = root / "junto-release"
            tool.write_text("#!/usr/bin/env python3\nimport pathlib, sys\n"
                            "pathlib.Path(sys.argv[4]).write_text(' '.join(sys.argv[1:4]))\n")
            tool.chmod(0o755)
            output = windows.assemble(client, recover, "1.2.3", root / "out", epoch=1_700_000_000,
                                      build=fixtures.build_directory(root / "build", TRIPLE), installer=installer,
                                      release_tool=tool)
            written = (output / "Junto-1.2.3-windows-x86_64-setup.exe").read_text()
            self.assertEqual(written, f"setup {installer} {output / 'package-directory'}")

    def test_a_file_that_is_not_a_windows_program_is_refused(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            client = root / "junto.exe"
            client.write_bytes(b"\x7fELF")
            with self.assertRaisesRegex(ValueError, "not a Windows program"):
                windows.assemble(client, client, "1.2.3", root / "out", build=root, installer=client)
            self.assertFalse((root / "out").exists())


class WindowsIconTests(unittest.TestCase):
    def check(self, pictures):
        self.assertEqual(sorted(pictures), SIZES)
        for size in SIZES:
            with self.subTest(size):
                self.assertEqual(pictures[size], icons.png(icons.rendering(icons.TOOL, size)))

    def test_the_icon_file_holds_the_rendering_of_the_source_at_each_size(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = pathlib.Path(temporary) / "junto.ico"
            icons.run(icons.TOOL, "ico", path)
            self.check(icons.ico(path.read_bytes()))

    @unittest.skipUnless(resource_linker(), "the LLVM linker for Windows programs (the Linux build image has it)")
    def test_a_program_linked_with_the_resource_file_holds_the_icon_group(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            icons.run(icons.TOOL, "windows-resource", root / "junto.res")
            # A library of resources only: it needs no code and no library of the system.
            subprocess.run([resource_linker(), "/dll", "/noentry", "/machine:x64", f"/out:{root / 'icon.dll'}",
                            str(root / "junto.res")], check=True, capture_output=True)
            self.check(icons.program_icon((root / "icon.dll").read_bytes()))


if __name__ == "__main__":
    unittest.main()
