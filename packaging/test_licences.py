"""The licence file of the client packages (ADR 0801)."""
import os
import pathlib
import subprocess
import sys
import tempfile
import tomllib
import unittest
from unittest import mock

import fixtures
import licences
from configuration import architecture

TARGETS = ["aarch64-apple-darwin", "x86_64-pc-windows-msvc", "x86_64-unknown-linux-gnu"]
MIT = "Permission is hereby granted, free of charge"
APACHE = "TERMS AND CONDITIONS FOR USE, REPRODUCTION, AND DISTRIBUTION"
RUST = tomllib.loads((licences.ROOT / "rust-toolchain.toml").read_text(encoding="utf-8"))["toolchain"]["channel"]


def blocks(document):
    """Each licence text of a licence file, with the labels of the components that it applies to."""
    found = []
    for part in document.split(f"{licences.RULE}\nText ")[1:]:
        heading, text = part.split("\n" + "-" * 78 + "\n", 1)
        found.append((text.rstrip("\n"), [line.strip() for line in heading.splitlines()[1:]]))
    return found


def texts(document):
    return dict(blocks(document))


def crate_line(document, name):
    return next(line for line in document.splitlines() if line.startswith(f"{name} "))


def host_triple():
    system = {"darwin": "macos", "win32": "windows", "linux": "linux"}[sys.platform]
    return licences.target(system, architecture())


class ClientLicenceTests(unittest.TestCase):
    def test_every_crate_of_the_client_graph_is_listed_for_each_system(self):
        for triple in TARGETS:
            with self.subTest(triple), tempfile.TemporaryDirectory() as temporary:
                document = licences.document("1.2.3", triple, fixtures.build_directory(temporary, triple))
                expected = fixtures.client_graph(triple)
                self.assertGreater(len(expected), 400)
                self.assertEqual(licences.listed(document), expected)
                self.assertTrue(document.startswith(f"Junto 1.2.3 for {triple}: third-party licences\n"))

    def test_the_file_holds_the_vendored_copies_the_webrtc_library_the_standard_library_and_the_fonts(self):
        with tempfile.TemporaryDirectory() as temporary:
            triple = "aarch64-apple-darwin"
            document = licences.document("1.2.3", triple, fixtures.build_directory(temporary, triple))
        found = texts(document)
        for name in ["livekit", "webrtc-sys", "webrtc-sys-build"]:
            self.assertIn("| changed copy in vendor/", crate_line(document, name))
        [webrtc] = [labels for text, labels in found.items() if text == fixtures.WEBRTC_LICENCE.decode().strip()]
        self.assertTrue(webrtc[0].startswith("libwebrtc webrtc-"), webrtc)
        notice = (licences.ROOT / "vendor/webrtc-sys-0.3.48/NOTICE.md").read_text(encoding="utf-8").strip()
        self.assertIn("webrtc-sys 0.3.48, NOTICE.md", found[notice])
        self.assertIn(f"Rust {RUST}, standard text of MIT", found[licences.normal(licences.standard_text("MIT"))])
        for licence in sorted((licences.ROOT / "crates/client/assets/fonts").glob("*-OFL.txt")):
            labels = found[licences.normal(licence.read_bytes())]
            self.assertTrue(labels[0].endswith(f": {licence.name}"), labels)
        apache = licences.normal((licences.ROOT / "LICENSE").read_bytes())
        self.assertEqual([text for text, _ in blocks(document)].count(apache), 1, "the Apache text is printed once")
        self.assertIn("livekit 0.9.4, standard text of Apache-2.0", found[apache])

    def test_the_webrtc_licence_of_this_build_is_found(self):
        triple = host_triple()
        document = licences.document("1.2.3", triple, licences.DEBUG_BUILD)
        [webrtc] = [text for text, labels in texts(document).items() if labels[0].startswith("libwebrtc ")]
        self.assertTrue(webrtc.startswith("# webrtc\n"), webrtc[:200])
        self.assertIn("Copyright (c) 2011, The WebRTC project authors", webrtc)

    def test_a_build_without_the_webrtc_licence_is_refused(self):
        with tempfile.TemporaryDirectory() as temporary:
            with self.assertRaisesRegex(ValueError, "licence file of the WebRTC library webrtc-.* for "
                                                    "x86_64-pc-windows-msvc is not under"):
                licences.document("1.2.3", "x86_64-pc-windows-msvc", temporary)
            fixtures.build_directory(temporary, "x86_64-pc-windows-msvc", profile="release")
            fixtures.build_directory(temporary, "x86_64-pc-windows-msvc", b"another text\n", profile="debug")
            with self.assertRaisesRegex(ValueError, "WebRTC licence files under the build differ"):
                licences.document("1.2.3", "x86_64-pc-windows-msvc", temporary)

    def test_a_webrtc_library_of_lk_custom_webrtc_gives_its_own_licence(self):
        with tempfile.TemporaryDirectory() as temporary:
            pathlib.Path(temporary, "LICENSE.md").write_text("# webrtc\nA custom build.\n")
            with mock.patch.dict(os.environ, {"LK_CUSTOM_WEBRTC": temporary}):
                document = licences.document("1.2.3", "x86_64-unknown-linux-gnu", temporary)
        self.assertEqual(texts(document)["# webrtc\nA custom build."], ["libwebrtc, LICENSE.md"])


def crate(directory, name, licence, files=None, extra=""):
    directory.mkdir(parents=True)
    (directory / "src").mkdir()
    (directory / "src/lib.rs").write_text("")
    authors = 'authors = ["Ada Example <ada@example.test>"]\n'
    (directory / "Cargo.toml").write_text(f'[package]\nname = "{name}"\nversion = "1.0.0"\nedition = "2021"\n'
                                          f'license = "{licence}"\n{authors}{extra}')
    for file, text in (files or {}).items():
        (directory / file).write_bytes(text.encode())


class WorkspaceTests(unittest.TestCase):
    """The generation from a small workspace of path crates."""

    def workspace(self, root, gamma_licence="MIT"):
        root = pathlib.Path(root)
        (root / "Cargo.toml").write_text('[workspace]\nmembers = ["app"]\nexclude = ["vendor", "elsewhere"]\n'
                                        'resolver = "2"\n')
        vendor = root / "vendor"
        crate(root / "app", "app", "Apache-2.0", extra=(
            '[dependencies]\nalpha = { path = "../vendor/alpha" }\nbeta = { path = "../vendor/beta" }\n'
            'gamma = { path = "../vendor/gamma" }\n'
            '[build-dependencies]\nepsilon = { path = "../vendor/epsilon" }\n'
            '[dev-dependencies]\ndelta = { path = "../vendor/delta" }\n'
            "[target.'cfg(windows)'.dependencies]\nzeta = { path = \"../vendor/zeta\" }\n"))
        same = "MIT License\n\nCopyright (c) Ada Example\n\n" + MIT + "...\n"
        crate(vendor / "alpha", "alpha", "MIT", {"LICENSE-MIT": same})
        crate(vendor / "beta", "beta", "MIT OR Apache-2.0", {"NOTICE": "Beta notice\n"})
        crate(vendor / "gamma", "gamma", gamma_licence, {"COPYING": same.replace("\n", "\r\n") + "  \n"})
        crate(vendor / "delta", "delta", "MIT", {"LICENSE": "Delta\n"})
        crate(vendor / "epsilon", "epsilon", "Zlib", {"license.txt": "Epsilon licence\n"})
        crate(vendor / "zeta", "zeta", "MIT", {"LICENSE": "Zeta\n"})
        subprocess.run(["cargo", "generate-lockfile", "--offline", "--manifest-path", str(root / "Cargo.toml")],
                       check=True, capture_output=True, timeout=300)
        return root

    def generate(self, root, triple="aarch64-unknown-linux-gnu"):
        return licences.document("1.2.3", triple, root, workspace=root, programs=["app"])

    def test_the_list_holds_normal_and_build_dependencies_of_the_target_and_no_test_dependencies(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = self.workspace(temporary)
            self.assertEqual({name for name, _ in licences.listed(self.generate(root))},
                             {"app", "alpha", "beta", "gamma", "epsilon"})
            self.assertEqual({name for name, _ in licences.listed(self.generate(root, "x86_64-pc-windows-msvc"))},
                             {"app", "alpha", "beta", "gamma", "epsilon", "zeta"})

    def test_texts_come_from_the_files_of_a_crate_or_from_the_standard_texts_and_each_is_printed_once(self):
        with tempfile.TemporaryDirectory() as temporary:
            document = self.generate(self.workspace(temporary))
        found = texts(document)
        self.assertEqual(crate_line(document, "app"), "app 1.0.0 | Apache-2.0 | Junto | Junto-LICENSE.txt")
        same = [labels for text, labels in found.items() if text.startswith("MIT License\n\nCopyright (c) Ada")]
        self.assertEqual(same, [["alpha 1.0.0, LICENSE-MIT", "gamma 1.0.0, COPYING"]])
        mit, apache = (licences.normal(licences.standard_text(name)) for name in ["MIT", "Apache-2.0"])
        self.assertIn("beta 1.0.0, standard text of MIT", found[mit])
        self.assertIn("beta 1.0.0, standard text of Apache-2.0", found[apache])
        self.assertTrue(found[apache][0].startswith("beta"), "a text is numbered by its first use")
        self.assertIn(APACHE, apache)
        self.assertEqual(found["Beta notice"], ["beta 1.0.0, NOTICE"])
        self.assertEqual(found["Epsilon licence"], ["epsilon 1.0.0, license.txt"])
        self.assertTrue(crate_line(document, "beta").endswith("; authors: Ada Example <ada@example.test>"))
        self.assertIn("| changed copy in vendor/alpha of the Junto source;", crate_line(document, "alpha"))

    def test_a_crate_without_a_licence_file_and_without_a_standard_text_is_refused(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = self.workspace(temporary)
            (root / "vendor/gamma/COPYING").unlink()
            (root / "vendor/gamma/Cargo.toml").write_text(
                (root / "vendor/gamma/Cargo.toml").read_text().replace('license = "MIT"',
                                                                       'license = "MIT AND Example-1.0"'))
            with self.assertRaisesRegex(ValueError, "gamma 1.0.0 holds no licence file, and there is no standard "
                                                    "text of Example-1.0"):
                self.generate(root)

    def test_a_path_crate_outside_the_workspace_and_vendor_is_refused(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = self.workspace(temporary)
            (root / "vendor").rename(root / "elsewhere")
            manifest = root / "app/Cargo.toml"
            manifest.write_text(manifest.read_text().replace("../vendor/", "../elsewhere/"))
            subprocess.run(["cargo", "generate-lockfile", "--offline", "--manifest-path", str(root / "Cargo.toml")],
                           check=True, capture_output=True, timeout=300)
            with self.assertRaisesRegex(ValueError, "alpha 1.0.0 comes from .*elsewhere.*neither crates.io"):
                self.generate(root)

    def test_licence_identifiers_of_an_expression(self):
        self.assertEqual(licences.identifiers("MIT/Apache-2.0"), ["MIT", "Apache-2.0"])
        self.assertEqual(licences.identifiers("(MIT OR Apache-2.0) AND Unicode-3.0"),
                         ["MIT", "Apache-2.0", "Unicode-3.0"])
        self.assertEqual(licences.identifiers("Apache-2.0 WITH LLVM-exception OR MIT"),
                         ["Apache-2.0", "LLVM-exception", "MIT"])


if __name__ == "__main__":
    unittest.main()
