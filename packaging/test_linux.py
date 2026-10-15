"""The parts of the Linux packaging that need no Linux build image."""
import os
import pathlib
import subprocess
import tarfile
import tempfile
import unittest

import linux

LDD = """\tlinux-vdso.so.1 (0x0000ffff9ba6c000)
\tlibxkbcommon.so.0 => /lib/x86_64-linux-gnu/libxkbcommon.so.0 (0x0000ffff93b80000)
\tlibxcb.so.1 => /lib/x86_64-linux-gnu/libxcb.so.1 (0x0000ffff93b70000)
\tlibpipewire-0.3.so.0 => /lib/x86_64-linux-gnu/libpipewire-0.3.so.0 (0x0000ffff93a80000)
\tlibc.so.6 => /lib/x86_64-linux-gnu/libc.so.6 (0x0000ffff936c0000)
\t/lib64/ld-linux-x86-64.so.2 (0x0000ffff9ba30000)
"""


class LinuxPackageTests(unittest.TestCase):
    def test_libraries_are_bundled_or_taken_from_the_host_by_name(self):
        found = linux.parse_ldd(LDD)
        self.assertEqual(linux.classify(found), {"libxkbcommon.so.0": "/lib/x86_64-linux-gnu/libxkbcommon.so.0"})
        with self.assertRaisesRegex(ValueError, "neither bundled nor taken from the host: libvulkan.so.1"):
            linux.classify(linux.parse_ldd(LDD + "\tlibvulkan.so.1 => /lib/libvulkan.so.1 (0x0000ffff93b80000)\n"))
        with self.assertRaisesRegex(ValueError, "does not find: libxkbcommon.so.0"):
            linux.classify(linux.parse_ldd(LDD.replace("/lib/x86_64-linux-gnu/libxkbcommon.so.0 (0x0000ffff93b80000)",
                                                       "not found")))
        with self.assertRaisesRegex(ValueError, "unexpected ldd line"):
            linux.parse_ldd("statically linked\n")
        self.assertFalse(linux.BUNDLED & linux.HOST)

    @unittest.skipIf(os.name == "nt", "the Linux packages are made on Linux")
    def test_the_tarball_holds_the_directory_as_junto_owned_by_root_at_one_time(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            directory = root / "tree"
            (directory / "lib").mkdir(parents=True)
            (directory / "junto").write_bytes(b"program")
            (directory / "junto").chmod(0o755)
            (directory / "lib/libxkbcommon.so.0").write_bytes(b"library")
            linux.tarball(directory, root / "package.tar.gz", 1_700_000_000)
            with tarfile.open(root / "package.tar.gz") as archive:
                members = archive.getmembers()
            self.assertEqual([member.name for member in members], ["junto", "junto/junto", "junto/lib",
                                                                   "junto/lib/libxkbcommon.so.0"])
            self.assertTrue(all(member.uid == 0 and member.uname == "root" for member in members))
            self.assertTrue(all(member.mtime == 1_700_000_000 for member in members))
            self.assertEqual(members[1].mode & 0o777, 0o755)

    def test_the_description_names_the_package_and_its_target(self):
        self.assertEqual(linux.description("1.2.3", "appimage", "x86_64"),
                         '{"format": 1, "version": "1.2.3", "system": "linux", "package": "appimage", '
                         '"architecture": "x86_64"}\n')

    @unittest.skipIf(os.name == "nt", "POSIX shell")
    def test_the_menu_entry_names_the_absolute_path_of_the_copy(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            copy = root / "copy with spaces/junto"
            linux.write(copy / f"share/applications/{linux.APPLICATION_ID}.desktop", linux.DESKTOP)
            linux.write(copy / f"share/icons/hicolor/scalable/apps/{linux.APPLICATION_ID}.svg", linux.SVG)
            linux.write(copy / "install-menu-entry", linux.INSTALL_ENTRY, 0o755)
            data = root / "data"
            subprocess.run([str(copy / "install-menu-entry")], check=True, capture_output=True,
                           env=dict(os.environ, XDG_DATA_HOME=str(data)))
            entry = (data / f"applications/{linux.APPLICATION_ID}.desktop").read_text()
            self.assertIn(f'Exec="{copy}/junto"\n', entry)
            self.assertIn(f"StartupWMClass={linux.APPLICATION_ID}\n", entry)
            self.assertTrue((data / f"icons/hicolor/scalable/apps/{linux.APPLICATION_ID}.svg").is_file())


if __name__ == "__main__":
    unittest.main()
