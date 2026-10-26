"""Bundle construction tests; no application is started and no identity is imported."""
import importlib.util
import json
import pathlib
import plistlib
import shutil
import subprocess
import sys
import tempfile
import unittest

import fixtures
import icons
import licences
from configuration import architecture

# The entries of the macOS icon file and the side of each picture (ADR 0725).
ICNS = {"ic04": 16, "ic11": 32, "ic05": 32, "ic12": 64, "ic07": 128, "ic13": 256, "ic08": 256, "ic14": 512,
        "ic09": 512, "ic10": 1024}
TRIPLE = licences.target("macos", architecture())


def load():
    spec = importlib.util.spec_from_file_location("bundle", pathlib.Path(__file__).with_name("bundle.py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class BundleTests(unittest.TestCase):
    def test_the_bundle_holds_the_compiled_icon_composer_icon_and_names_it(self):
        module = load()
        with tempfile.TemporaryDirectory(prefix="junto-bundle-") as temporary:
            root = pathlib.Path(temporary)
            binary = root / "junto"
            binary.write_bytes(b"program")
            compiled = []

            def compiler(document, resources):
                compiled.append(document)
                (resources / "Assets.car").write_bytes(b"catalog")

            bundle = module.assemble(binary, root / "Junto.app", "0.2.3", icon_tool=icons.TOOL,
                                     build=fixtures.build_directory(root / "build", TRIPLE), icon_compiler=compiler)
            info = plistlib.loads((bundle / "Contents/Info.plist").read_bytes())
            self.assertEqual(info["CFBundleIconName"], "Junto")
            self.assertEqual(info["CFBundleIconFile"], "Junto.icns")
            self.assertEqual((bundle / "Contents/Resources/Assets.car").read_bytes(), b"catalog")
            self.assertEqual(compiled, [module.ICON_DOCUMENT])
        # The document names its layer, which is in its assets.
        document = json.loads((module.ICON_DOCUMENT / "icon.json").read_text())
        names = [layer["image-name"] for group in document["groups"] for layer in group["layers"]]
        self.assertEqual(names, ["discs.svg"])
        for name in names:
            self.assertTrue((module.ICON_DOCUMENT / "Assets" / name).is_file())


    def test_bundle_uses_the_requested_workspace_version_and_permission_texts(self):
        module = load()
        with tempfile.TemporaryDirectory(prefix="junto-bundle-") as temporary:
            root = pathlib.Path(temporary)
            binary = root / "junto"
            binary.write_bytes(b"not executable in this test")
            bundle = module.assemble(binary, root / "Junto.app", "0.2.3", icon_tool=icons.TOOL,
                                     build=fixtures.build_directory(root / "build", TRIPLE))
            info = plistlib.loads((bundle / "Contents/Info.plist").read_bytes())
            self.assertEqual(info["CFBundleIdentifier"], "org.junto.app")
            self.assertEqual(info["CFBundleVersion"], "0.2.3")
            self.assertEqual(info["CFBundleShortVersionString"], "0.2.3")
            self.assertEqual(info["CFBundleExecutable"], "junto")
            for item in ["NSMicrophoneUsageDescription", "NSCameraUsageDescription", "NSScreenCaptureUsageDescription",
                         "NSDownloadsFolderUsageDescription", "NSLocalNetworkUsageDescription"]:
                self.assertTrue(info[item])
            self.assertTrue((bundle / "Contents/Resources/Junto.icns").is_file())
            self.assertEqual((bundle / "Contents/MacOS/junto").read_bytes(), binary.read_bytes())
            with self.assertRaises(FileExistsError):
                module.assemble(binary, bundle, "0.2.4", icon_tool=icons.TOOL, build=root / "build")

    def test_the_bundle_holds_the_licence_of_junto_and_the_licence_file_of_its_programs(self):
        with tempfile.TemporaryDirectory(prefix="junto-bundle-") as temporary:
            root = pathlib.Path(temporary)
            binary = root / "junto"
            binary.write_bytes(b"not executable in this test")
            bundle = load().assemble(binary, root / "Junto.app", "0.2.3", root / "junto", icon_tool=icons.TOOL,
                                     build=fixtures.build_directory(root / "build", TRIPLE))
            folder = bundle / "Contents/Resources/licences"
            self.assertEqual(sorted(path.name for path in folder.iterdir()), ["Junto-LICENSE.txt", licences.NAME])
            self.assertEqual((folder / "Junto-LICENSE.txt").read_bytes(), (licences.ROOT / "LICENSE").read_bytes())
            text = (folder / licences.NAME).read_text(encoding="utf-8")
            self.assertTrue(text.startswith(f"Junto 0.2.3 for {TRIPLE}: third-party licences\n"))
            self.assertEqual(licences.listed(text), fixtures.client_graph(TRIPLE))
            self.assertIn(fixtures.WEBRTC_LICENCE.decode(), text)

    def test_the_icon_file_holds_the_rendering_of_the_source_at_each_size_of_macos(self):
        with tempfile.TemporaryDirectory(prefix="junto-bundle-") as temporary:
            root = pathlib.Path(temporary)
            binary = root / "junto"
            binary.write_bytes(b"not executable in this test")
            bundle = load().assemble(binary, root / "Junto.app", "0.2.3", icon_tool=icons.TOOL,
                                     build=fixtures.build_directory(root / "build", TRIPLE))
            self.assertEqual(plistlib.loads((bundle / "Contents/Info.plist").read_bytes())["CFBundleIconFile"],
                             "Junto.icns")
            entries = icons.icns((bundle / "Contents/Resources/Junto.icns").read_bytes())
            self.assertEqual(sorted(entries), sorted(ICNS))
            for kind, size in ICNS.items():
                with self.subTest(kind):
                    rendering = icons.rendering(icons.TOOL, size)
                    if entries[kind][:8] == icons.PNG_SIGNATURE:
                        self.assertEqual(entries[kind], rendering)
                    else:
                        picture, expected = icons.icns_picture(entries[kind], size), icons.png(rendering)
                        self.assertLessEqual(picture.difference(expected), 1)
                        self.assertEqual(picture.rgba[3::4], expected.rgba[3::4])

    @unittest.skipUnless(sys.platform == "darwin" and shutil.which("iconutil"),
                         "requires macOS and its iconutil program")
    def test_argb_edges_match_what_macos_writes_from_the_same_pictures(self):
        with tempfile.TemporaryDirectory(prefix="junto-bundle-") as temporary:
            root = pathlib.Path(temporary)
            binary = root / "junto"
            binary.write_bytes(b"not executable in this test")
            bundle = load().assemble(binary, root / "Junto.app", "0.2.3", icon_tool=icons.TOOL,
                                     build=fixtures.build_directory(root / "build", TRIPLE))
            entries = icons.icns((bundle / "Contents/Resources/Junto.icns").read_bytes())
            iconset = root / "reference.iconset"
            iconset.mkdir()
            for side in [16, 32, 128, 256, 512]:
                for suffix, scale in [("", 1), ("@2x", 2)]:
                    (iconset / f"icon_{side}x{side}{suffix}.png").write_bytes(
                        icons.rendering(icons.TOOL, side * scale))
            reference = root / "reference.icns"
            subprocess.run(["iconutil", "-c", "icns", "-o", str(reference), str(iconset)],
                           check=True, capture_output=True)
            expected_entries = icons.icns(reference.read_bytes())
            for kind, size in [("ic04", 16), ("ic05", 32)]:
                with self.subTest(kind):
                    self.assertEqual(entries[kind][:4], b"ARGB")
                    self.assertEqual(expected_entries[kind][:4], b"ARGB")
                    actual = icons.icns_picture(entries[kind], size)
                    expected = icons.icns_picture(expected_entries[kind], size)
                    rendering = icons.png(icons.rendering(icons.TOOL, size))
                    edges = [(x, y) for y in range(size) for x in range(size)
                             if 0 < rendering.pixel(x, y)[3] < 255]
                    self.assertTrue(edges, "the picture has no partly transparent pixels")
                    for x, y in edges:
                        self.assertEqual(actual.pixel(x, y), expected.pixel(x, y), (kind, x, y))

    @unittest.skipUnless(sys.platform == "darwin" and shutil.which("iconutil"), "iconutil of macOS")
    def test_macos_reads_every_size_of_the_icon_file(self):
        with tempfile.TemporaryDirectory(prefix="junto-bundle-") as temporary:
            root = pathlib.Path(temporary)
            binary = root / "junto"
            binary.write_bytes(b"not executable in this test")
            bundle = load().assemble(binary, root / "Junto.app", "0.2.3", icon_tool=icons.TOOL,
                                     build=fixtures.build_directory(root / "build", TRIPLE))
            iconset = root / "Junto.iconset"
            subprocess.run(["iconutil", "-c", "iconset", "-o", str(iconset),
                            str(bundle / "Contents/Resources/Junto.icns")], check=True, capture_output=True)
            names = {f"icon_{side}x{side}{scale}.png" for side in [16, 32, 128, 256, 512] for scale in ["", "@2x"]}
            self.assertEqual({path.name for path in iconset.iterdir()}, names)
            # iconutil writes the pictures again; its values differ by at most one step. From an
            # ARGB entry (16 and 32 pixels) it writes other colours for pixels that are partly
            # transparent, also from the icon file that iconutil itself writes from the same
            # pictures; there the alpha of every pixel and the colour of the opaque ones are compared.
            for side, scale, size, whole in [(16, "", 16, False), (32, "", 32, False), (32, "@2x", 64, True)]:
                name = f"icon_{side}x{side}{scale}.png"
                with self.subTest(name):
                    read = icons.png((iconset / name).read_bytes())
                    rendering = icons.png(icons.rendering(icons.TOOL, size))
                    if whole:
                        self.assertLessEqual(read.difference(rendering), 1)
                    for y in range(size):
                        for x in range(size):
                            pixel, expected = read.pixel(x, y), rendering.pixel(x, y)
                            compared = range(4) if expected[3] == 255 else [3]
                            self.assertLessEqual(max(abs(pixel[i] - expected[i]) for i in compared), 1, (x, y))


if __name__ == "__main__":
    unittest.main()
