"""The parts of the Linux packaging that need no Linux build image."""
import io
import os
import pathlib
import subprocess
import tarfile
import tempfile
import unittest

from unittest import mock

import icons
import linux

# The sizes of the hicolor icon theme that the packages hold (ADR 0754).
SIZES = [16, 22, 24, 32, 48, 64, 128, 256, 512]

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
            linux.icon_theme(copy, icons.TOOL)
            linux.write(copy / "install-menu-entry", linux.INSTALL_ENTRY, 0o755)
            data = root / "data"
            subprocess.run([str(copy / "install-menu-entry")], check=True, capture_output=True,
                           env=dict(os.environ, XDG_DATA_HOME=str(data)))
            entry = (data / f"applications/{linux.APPLICATION_ID}.desktop").read_text()
            self.assertIn(f'Exec="{copy}/junto"\n', entry)
            self.assertIn(f"StartupWMClass={linux.APPLICATION_ID}\n", entry)
            for size in [*(f"{size}x{size}" for size in SIZES), "scalable"]:
                ending = "svg" if size == "scalable" else "png"
                self.assertEqual((data / f"icons/hicolor/{size}/apps/{linux.APPLICATION_ID}.{ending}").read_bytes(),
                                 (copy / f"share/icons/hicolor/{size}/apps/{linux.APPLICATION_ID}.{ending}").read_bytes())

    def test_the_icon_theme_holds_the_source_and_its_rendering_at_each_size(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = pathlib.Path(temporary) / "junto"
            linux.icon_theme(directory, icons.TOOL)
            theme = directory / "share/icons/hicolor"
            self.assertEqual(sorted(path.name for path in theme.iterdir()),
                             sorted([*(f"{size}x{size}" for size in SIZES), "scalable"]))
            self.assertEqual((theme / f"scalable/apps/{linux.APPLICATION_ID}.svg").read_bytes(),
                             icons.SOURCE.read_bytes())
            for size in SIZES:
                with self.subTest(size):
                    self.assertEqual([path.name for path in (theme / f"{size}x{size}/apps").iterdir()],
                                     [f"{linux.APPLICATION_ID}.png"])
                    picture = (theme / f"{size}x{size}/apps/{linux.APPLICATION_ID}.png").read_bytes()
                    self.assertEqual(picture, icons.rendering(icons.TOOL, size))
                    self.assertEqual(icons.png(picture).size, size)


class RuntimeTests(unittest.TestCase):
    """The AppImage runtime and the source that the AppImage carries (ADR 0754, ADR 0801)."""

    def test_the_runtime_section_holds_the_licence_texts_of_each_component_from_its_source_archive(self):
        with tempfile.TemporaryDirectory() as temporary:
            source = pathlib.Path(temporary) / "runtime-source"
            (source / "sources").mkdir(parents=True)
            for _, archive, names in linux.RUNTIME_COMPONENTS:
                top = archive.removesuffix(".tar.gz").removesuffix(".tar.xz")
                mode = "w:xz" if archive.endswith(".xz") else "w:gz"
                with tarfile.open(source / "sources" / archive, mode) as output:
                    for name in [*names, "README"]:
                        content = f"{name} of {top}\n".encode()
                        info = tarfile.TarInfo(f"{top}/{name}")
                        info.size = len(content)
                        output.addfile(info, io.BytesIO(content))
            packages = []
            for name in ["musl", "compiler-rt"]:
                path = pathlib.Path(temporary) / f"{name}-copyright"
                path.write_text(f"{name} copyright\n")
                packages.append((name, path))
            with mock.patch.object(linux, "RUNTIME_PACKAGES", packages):
                section = linux.runtime_section(source)
        found = dict((text.decode().strip(), label) for label, text in section.components)
        self.assertEqual(found["LGPL2.txt of fuse-3.15.0"],
                         "libfuse 3.15.0, with the change of the AppImage project: LGPL2.txt")
        self.assertEqual(len(found), sum(len(names) for _, _, names in linux.RUNTIME_COMPONENTS) + 2)
        self.assertEqual(found["musl copyright"], f"musl: {packages[0][1]}")
        self.assertEqual(found["compiler-rt copyright"], f"compiler-rt: {packages[1][1]}")
        self.assertNotIn("README of zlib-1.3.1", found)
        description = " ".join(section.description)
        self.assertIn("GNU Lesser General Public License 2.1", description)
        self.assertIn("licences/appimage-runtime", description)
        self.assertIn("relink", description)

    def test_the_link_script_links_only_files_of_its_directory_and_the_instructions_name_each(self):
        tools = pathlib.Path(linux.__file__).resolve().parent.parent / "tools/linux/appimage-runtime"
        named = linux.link_inputs((tools / "link.sh").read_text())
        self.assertEqual(named, ["Scrt1.o", "crti.o", "clang_rt.crtbegin.o", "runtime.o", "libsquashfuse.a",
                                 "libsquashfuse_ll.a", "libzstd.a", "libz.a", "libfuse3.a", "libmimalloc.a",
                                 "libclang_rt.builtins.a", "libc.a", "clang_rt.crtend.o", "crtn.o",
                                 "data_sections.ld"])
        self.assertNotIn("/", " ".join(named))
        self.assertNotRegex((tools / "link.sh").read_text(), r"\s-[lL]\S")
        readme = (tools / "README.md").read_text()
        for name in [*named, "link.sh"]:
            with self.subTest(name):
                self.assertIn(f"`{name}`", readme)
        build = (tools / "build.sh").read_text()
        self.assertIn('sh "$relink/link.sh" "$output"', build)
        self.assertNotRegex(build, r"\$CC [^\n]*-o runtime\b")

    def test_the_paths_of_the_build_image_are_posix_paths_on_every_system(self):
        for path in [linux.RUNTIME, linux.RUNTIME_SOURCE, *(path for _, path in linux.RUNTIME_PACKAGES)]:
            with self.subTest(str(path)):
                self.assertIs(type(path), pathlib.PurePosixPath)
                self.assertTrue(str(path).startswith("/"))
                self.assertNotIn("\\", str(path))

    def test_the_link_inputs_are_read_from_the_ld_command(self):
        script = "set -eu\nld -static -o \"$output\" \\\n    a.o libb.a \\\n    -T c.ld\nstrip x.o\n"
        self.assertEqual(linux.link_inputs(script), ["a.o", "libb.a", "c.ld"])
        with self.assertRaisesRegex(ValueError, "no ld command"):
            linux.link_inputs("strip a.o\n")

    def test_the_build_image_downloads_and_the_build_script_extracts_the_archive_of_each_component(self):
        tools = pathlib.Path(linux.__file__).resolve().parent.parent / "tools/linux"
        dockerfile = (tools / "Dockerfile").read_text()
        script = (tools / "appimage-runtime/build.sh").read_text()
        readme = (tools / "appimage-runtime/README.md").read_text()
        for _, archive, _ in linux.RUNTIME_COMPONENTS:
            with self.subTest(archive):
                top = archive.removesuffix(".tar.gz").removesuffix(".tar.xz")
                self.assertRegex(dockerfile, f"-o {archive} ")
                self.assertRegex(dockerfile, f"[0-9a-f]{{64}} {archive} ")
                self.assertIn(top.removeprefix("type2-runtime-"), script)
                self.assertIn(archive, readme)
        self.assertIn("COPY appimage-runtime/README.md appimage-runtime/build.sh appimage-runtime/link.sh \\\n"
                      f"    {linux.RUNTIME_SOURCE}/", dockerfile)
        self.assertIn(f"build.sh {linux.RUNTIME} \\\n        {linux.RUNTIME_SOURCE}/relink", dockerfile)


if __name__ == "__main__":
    unittest.main()
