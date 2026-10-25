"""The licence file of the client packages (ADR 0801)."""
import os
import re
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

TEST_TEXTS = pathlib.Path(__file__).with_name("test-licence-texts")

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


def numbers(document, text):
    """The number of a licence text in a licence file."""
    key = licences.normal(text)
    [number] = [index for index, (block, _) in enumerate(blocks(document), start=1) if block == key]
    return number


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

    def test_the_texts_of_each_crate_give_the_licences_that_its_expression_requires(self):
        for triple in TARGETS:
            with self.subTest(triple), tempfile.TemporaryDirectory() as temporary:
                document = licences.document("1.2.3", triple, fixtures.build_directory(temporary, triple))
                self.assertEqual(licences.uncovered(document), [])

    def test_the_file_holds_the_vendored_copies_the_webrtc_library_the_standard_library_and_the_fonts(self):
        with tempfile.TemporaryDirectory() as temporary:
            triple = "aarch64-apple-darwin"
            document = licences.document("1.2.3", triple, fixtures.build_directory(temporary, triple))
        found = texts(document)
        for name in ["cpal", "livekit", "webrtc-sys", "webrtc-sys-build"]:
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


def crate(directory, name, licence, files=None, extra="", authors='authors = ["Ada Example <ada@example.test>"]\n'):
    directory.mkdir(parents=True)
    (directory / "src").mkdir()
    (directory / "src/lib.rs").write_text("")
    (directory / "Cargo.toml").write_text(f'[package]\nname = "{name}"\nversion = "1.0.0"\nedition = "2021"\n'
                                          f'license = "{licence}"\n{authors}{extra}')
    for file, text in (files or {}).items():
        (directory / file).write_bytes(text.encode())


def standard(identifier, holder="Ada Example"):
    """The standard text of `identifier` with a copyright line."""
    text = licences.standard_text(identifier).decode()
    return text.replace("<year> <copyright holders>", f"2024 {holder}")


BSD_3 = """Copyright (c) 2009 Example Authors

Redistribution and use in source and binary forms, with or without modification, are permitted provided that
the following conditions are met:
1. Redistributions of source code must retain the above copyright notice.
2. Redistributions in binary form must reproduce the above copyright notice.
3. Neither the name of the copyright holder nor the names of its contributors may be used to endorse or promote
products derived from this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES
ARE DISCLAIMED.
"""


class WorkspaceTests(unittest.TestCase):
    """The generation from a small workspace of path crates."""

    def workspace(self, root, gamma_licence="MIT"):
        root = pathlib.Path(root)
        (root / "Cargo.toml").write_text('[workspace]\nmembers = ["app"]\nexclude = ["vendor", "elsewhere"]\n'
                                        'resolver = "2"\n')
        vendor = root / "vendor"
        crate(root / "app", "app", "Apache-2.0", extra=(
            '[dependencies]\nalpha = { path = "../vendor/alpha" }\nbeta = { path = "../vendor/beta" }\n'
            'gamma = { path = "../vendor/gamma" }\ntheta = { path = "../vendor/theta" }\n'
            'iota = { path = "../vendor/iota" }\n'
            '[build-dependencies]\nepsilon = { path = "../vendor/epsilon" }\n'
            '[dev-dependencies]\ndelta = { path = "../vendor/delta" }\n'
            "[target.'cfg(windows)'.dependencies]\nzeta = { path = \"../vendor/zeta\" }\n"))
        same = standard("MIT")
        crate(vendor / "alpha", "alpha", "MIT", {"LICENSE-MIT": same, "LICENSE-THIRD-PARTY": BSD_3})
        crate(vendor / "beta", "beta", "MIT OR Apache-2.0", {"NOTICE": "Beta notice\n"})
        crate(vendor / "gamma", "gamma", gamma_licence, {"COPYING": same.replace("\n", "\r\n") + "  \n"})
        crate(vendor / "delta", "delta", "MIT", {"LICENSE": "Delta\n"})
        crate(vendor / "epsilon", "epsilon", "Zlib", {"license.txt": "Epsilon is under the zlib licence.\n"})
        crate(vendor / "zeta", "zeta", "MIT", {"LICENSE": "Zeta\n"})
        crate(vendor / "theta", "theta", "Apache-2.0 AND MIT",
              {"LICENSE-APACHE": (licences.ROOT / "LICENSE").read_text()},
              authors='repository = "https://example.test/theta"\n')
        crate(vendor / "iota", "iota", "MIT OR Apache-2.0",
              {"LICENSE-APACHE": "../LICENSE-APACHE", "LICENSE-MIT": "../LICENSE-MIT"})
        subprocess.run(["cargo", "generate-lockfile", "--offline", "--manifest-path", str(root / "Cargo.toml")],
                       check=True, capture_output=True, timeout=300)
        return root

    def generate(self, root, triple="aarch64-unknown-linux-gnu"):
        return licences.document("1.2.3", triple, root, workspace=root,
                                 product=licences.Product(["app"], "the program app", "this package"))

    def test_the_list_holds_normal_and_build_dependencies_of_the_target_and_no_test_dependencies(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = self.workspace(temporary)
            self.assertEqual({name for name, _ in licences.listed(self.generate(root))},
                             {"app", "alpha", "beta", "gamma", "epsilon", "theta", "iota"})
            self.assertEqual({name for name, _ in licences.listed(self.generate(root, "x86_64-pc-windows-msvc"))},
                             {"app", "alpha", "beta", "gamma", "epsilon", "theta", "iota", "zeta"})

    def test_texts_come_from_the_files_of_a_crate_or_from_the_standard_texts_and_each_is_printed_once(self):
        with tempfile.TemporaryDirectory() as temporary:
            document = self.generate(self.workspace(temporary))
        found = texts(document)
        self.assertEqual(crate_line(document, "app"), "app 1.0.0 | Apache-2.0 | Junto | Junto-LICENSE.txt")
        same = [labels for text, labels in found.items() if text.startswith("MIT License\n\nCopyright (c) 2024 Ada")]
        self.assertEqual(same, [["alpha 1.0.0, LICENSE-MIT", "gamma 1.0.0, COPYING"]])
        mit, apache = (licences.normal(licences.standard_text(name)) for name in ["MIT", "Apache-2.0"])
        self.assertIn("beta 1.0.0, standard text of MIT", found[mit])
        self.assertNotIn("beta 1.0.0, standard text of Apache-2.0", found[apache])
        self.assertTrue(found[apache][0].startswith("theta"), "a text is numbered by its first use")
        self.assertIn(APACHE, apache)
        self.assertEqual(found["Beta notice"], ["beta 1.0.0, NOTICE"])
        self.assertEqual(found["Epsilon is under the zlib licence."], ["epsilon 1.0.0, license.txt"])
        self.assertIn("| changed copy in vendor/alpha of the Junto source;", crate_line(document, "alpha"))
        self.assertTrue(crate_line(document, "gamma").endswith("| texts 1"), crate_line(document, "gamma"))

    def test_a_crate_without_a_licence_file_names_the_standard_texts_and_the_declared_authors(self):
        with tempfile.TemporaryDirectory() as temporary:
            document = self.generate(self.workspace(temporary))
        mit = numbers(document, licences.standard_text("MIT"))
        self.assertEqual(crate_line(document, "beta").split(" | ", 3)[3],
                         f"texts {mit}, {numbers(document, b'Beta notice')}; the package holds no licence file: text "
                         f"{mit} is the standard text of MIT, a licence that its manifest declares, and the copyright "
                         "holders are the authors that its manifest declares: Ada Example <ada@example.test>")

    def test_licence_files_without_the_text_of_a_required_licence_are_reported_and_completed(self):
        with tempfile.TemporaryDirectory() as temporary:
            document = self.generate(self.workspace(temporary))
        zlib = numbers(document, licences.standard_text("Zlib"))
        self.assertEqual(crate_line(document, "epsilon").split(" | ", 3)[3],
                         f"texts {numbers(document, b'Epsilon is under the zlib licence.')}, {zlib}; the licence files "
                         "of the package do not hold the texts that its licence expression requires: text "
                         f"{zlib} is the standard text of Zlib, a licence that its manifest declares, and the "
                         "copyright holders are the authors that its manifest declares: Ada Example <ada@example.test>")
        mit = numbers(document, licences.standard_text("MIT"))
        apache = numbers(document, (licences.ROOT / "LICENSE").read_bytes())
        self.assertEqual(crate_line(document, "theta").split(" | ", 3)[3],
                         f"texts {apache}, {mit}; the licence files of the package do not hold the texts that its "
                         f"licence expression requires: text {mit} is the standard text of MIT, a licence that its "
                         "manifest declares, and its manifest "
                         "declares no authors, and the copyright holders are the authors of the crate in its "
                         "repository https://example.test/theta")

    def test_licence_files_that_hold_only_a_path_get_the_standard_texts_of_each_alternative(self):
        with tempfile.TemporaryDirectory() as temporary:
            document = self.generate(self.workspace(temporary))
        mit = numbers(document, licences.standard_text("MIT"))
        paths = [numbers(document, f"../LICENSE-{name}".encode()) for name in ["APACHE", "MIT"]]
        self.assertEqual(crate_line(document, "iota").split(" | ", 3)[3],
                         f"texts {paths[0]}, {paths[1]}, {mit}; the licence files of the package do not hold the texts "
                         f"that its licence expression requires: text {mit} is the standard text of MIT, a licence "
                         "that its manifest declares, and the copyright holders are the authors that its manifest "
                         "declares: Ada Example <ada@example.test>")

    def test_a_licence_file_with_only_the_heading_of_a_licence_is_completed_with_a_standard_text(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = self.workspace(temporary, gamma_licence="MPL-2.0 OR MIT")
            (root / "vendor/gamma/COPYING").write_text("Mozilla Public License Version 2.0\n")
            document = self.generate(root)
        mit = numbers(document, licences.standard_text("MIT"))
        line = crate_line(document, "gamma")
        self.assertIn(f"text {mit} is the standard text of MIT, a licence that its manifest declares", line)
        self.assertEqual(licences.uncovered(document), [])

    def test_the_completion_adds_one_sufficient_alternative_of_each_or(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = self.workspace(temporary, gamma_licence="(MIT OR ISC) AND Zlib")
            document = self.generate(root)
        zlib = numbers(document, licences.standard_text("Zlib"))
        line = crate_line(document, "gamma")
        self.assertIn(f"| texts 1, {zlib}; ", line)
        self.assertIn(f"text {zlib} is the standard text of Zlib, a licence that its manifest declares", line)
        self.assertEqual(licences.uncovered(document), [])
        tree = licences.expression("(MIT OR ISC) AND (Zlib OR Apache-2.0)")
        self.assertEqual(licences.completion(tree, set(), {"MIT", "ISC", "Zlib", "Apache-2.0"}), ["MIT", "Zlib"])
        self.assertEqual(licences.completion(tree, {"ISC"}, {"Apache-2.0"}), ["Apache-2.0"])
        self.assertIsNone(licences.completion(tree, set(), {"MIT"}))
        self.assertEqual(licences.completion(licences.expression("Apache-2.0 WITH LLVM-exception OR MIT"),
                                             {"Apache-2.0"}, {"MIT", "LLVM-exception"}), ["LLVM-exception"])

    def test_licence_files_with_the_text_of_an_undeclared_licence_are_reported(self):
        with tempfile.TemporaryDirectory() as temporary:
            document = self.generate(self.workspace(temporary))
        self.assertTrue(crate_line(document, "alpha").endswith(
            "; its licence files also hold the text of BSD-3-Clause, which its manifest does not declare"),
            crate_line(document, "alpha"))
        self.assertNotIn("also hold", crate_line(document, "gamma"))

    def test_a_crate_without_a_licence_file_and_without_a_standard_text_is_refused(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = self.workspace(temporary)
            (root / "vendor/gamma/COPYING").unlink()
            (root / "vendor/gamma/Cargo.toml").write_text(
                (root / "vendor/gamma/Cargo.toml").read_text().replace('license = "MIT"',
                                                                       'license = "MIT AND Example-1.0"'))
            with self.assertRaisesRegex(ValueError, "the licence files of gamma 1.0.0 do not hold the texts that MIT "
                                                    "AND Example-1.0 requires, and there is no standard text of "
                                                    "Example-1.0"):
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



class ExpressionTests(unittest.TestCase):
    def test_licence_identifiers_of_an_expression(self):
        self.assertEqual(licences.identifiers("MIT/Apache-2.0"), ["MIT", "Apache-2.0"])
        self.assertEqual(licences.identifiers("MIT / Apache-2.0"), ["MIT", "Apache-2.0"])
        self.assertEqual(licences.identifiers("(MIT OR Apache-2.0) AND Unicode-3.0"),
                         ["MIT", "Apache-2.0", "Unicode-3.0"])
        self.assertEqual(licences.identifiers("Apache-2.0 WITH LLVM-exception OR MIT"),
                         ["Apache-2.0", "LLVM-exception", "MIT"])

    def test_the_tree_of_an_expression_binds_with_before_and_before_or(self):
        self.assertEqual(licences.expression("MIT OR Apache-2.0 AND Zlib"),
                         ("OR", ["MIT", ("AND", ["Apache-2.0", "Zlib"])]))
        self.assertEqual(licences.expression("(MIT OR Apache-2.0) AND Zlib"),
                         ("AND", [("OR", ["MIT", "Apache-2.0"]), "Zlib"]))
        self.assertEqual(licences.expression("Apache-2.0 WITH LLVM-exception OR MIT"),
                         ("OR", [("WITH", "Apache-2.0", "LLVM-exception"), "MIT"]))
        for text in ["", "MIT OR", "(MIT", "MIT)", "MIT Apache-2.0", "MIT WITH", "AND MIT"]:
            with self.subTest(text), self.assertRaises(ValueError):
                licences.expression(text)

    def test_an_expression_is_satisfied_by_one_alternative_each_part_and_a_licence_with_its_exception(self):
        cases = [("MIT OR Apache-2.0", {"MIT"}, True), ("MIT OR Apache-2.0", {"Zlib"}, False),
                 ("MIT AND Apache-2.0", {"MIT"}, False), ("MIT AND Apache-2.0", {"MIT", "Apache-2.0"}, True),
                 ("(MIT OR Apache-2.0) AND Unicode-3.0", {"Apache-2.0", "Unicode-3.0"}, True),
                 ("Apache-2.0 WITH LLVM-exception", {"Apache-2.0"}, False),
                 ("Apache-2.0 WITH LLVM-exception OR MIT", {"MIT"}, True)]
        for text, present, expected in cases:
            with self.subTest(text, present=present):
                self.assertEqual(licences.satisfied(licences.expression(text), present), expected)


class LicenceTextTests(unittest.TestCase):
    def test_each_standard_text_is_identified_as_its_own_licence_only(self):
        for path in sorted(licences.STANDARD_TEXTS.glob("*.txt")):
            with self.subTest(path.name):
                self.assertEqual(licences.licences_in(path.read_bytes()), {path.stem})
        self.assertEqual(licences.licences_in((licences.ROOT / "LICENSE").read_bytes()), {"Apache-2.0"})
        for licence in sorted((licences.ROOT / "crates/client/assets/fonts").glob("*-OFL.txt")):
            self.assertEqual(licences.licences_in(licence.read_bytes()), {"OFL-1.1"})

    def test_the_heading_of_a_licence_alone_is_no_licence_text(self):
        headings = ["Mozilla Public License Version 2.0", "Apache License\nVersion 2.0, January 2004",
                    "UNICODE LICENSE V3", "Creative Commons Legal Code\n\nCC0 1.0 Universal",
                    "SIL OPEN FONT LICENSE Version 1.1 - 26 February 2007",
                    "Boost Software License - Version 1.0 - August 17th, 2003",
                    "GNU LESSER GENERAL PUBLIC LICENSE\nVersion 2.1, February 1999",
                    "This is free and unencumbered software released into the public domain.",
                    "---- LLVM Exceptions to the Apache 2.0 License ----", "MIT License", "BSD 3-Clause License"]
        for heading in headings:
            with self.subTest(heading):
                self.assertEqual(licences.licences_in(heading.encode()), set())

    def test_the_headings_of_the_sections_of_a_licence_are_no_licence_text(self):
        mpl = (TEST_TEXTS / "MPL-2.0.txt").read_text()
        self.assertEqual(licences.licences_in(mpl.encode()), {"MPL-2.0"})
        # The lines of the title and of the headings, also those inside the box of sections 6 and 7.
        lines = [line.strip(" *") for line in mpl.splitlines()]
        headings = "\n".join(line for line in lines
                             if line.startswith("Mozilla Public License") or re.fullmatch(r"\d+(\.\d+)*\. \S.{0,60}", line))
        for heading in ("2.1. Grants", "6. Disclaimer of Warranty", "10. Versions of the License"):
            self.assertIn(heading, headings)
        cc0 = ("Creative Commons Legal Code\n\nCC0 1.0 Universal\n\nStatement of Purpose\n\n1. Copyright and Related "
               "Rights.\n\n2. Waiver.\n\n3. Public License Fallback.\n\n4. Limitations and Disclaimers.\n")
        for identifier, text in (("MPL-2.0", headings), ("CC0-1.0", cc0)):
            with self.subTest(identifier):
                self.assertEqual(licences.licences_in(text.encode()), set())

    def test_each_licence_is_identified_by_a_phrase_of_its_terms(self):
        for identifier, (required, _) in licences.LICENCE_PHRASES.items():
            with self.subTest(identifier):
                self.assertGreaterEqual(max(len(phrase.split()) for phrase in required), licences.OPERATIVE_WORDS)

    def test_variants_and_neighbours_of_a_licence_are_told_apart(self):
        mit = standard("MIT")
        variant = mit.replace("permission notice shall", "permission notice (including the next\nparagraph) shall")
        self.assertEqual(licences.licences_in(variant.encode()), {"MIT"})
        without_condition = mit.split("The above copyright")[0] + "THE SOFTWARE IS" + mit.split("THE SOFTWARE IS")[1]
        self.assertEqual(licences.licences_in(without_condition.encode()), {"MIT-0"})
        self.assertEqual(licences.licences_in(BSD_3.encode()), {"BSD-3-Clause"})
        self.assertEqual(licences.licences_in((BSD_3.split("3. Neither")[0] + "THIS SOFTWARE IS PROVIDED " +
                                               BSD_3.split("THIS SOFTWARE IS PROVIDED ")[1]).encode()),
                         {"BSD-2-Clause"})
        self.assertEqual(licences.licences_in((mit + "\n" + BSD_3).encode()), {"MIT", "BSD-3-Clause"})
        self.assertEqual(licences.licences_in(b"Licensed under the Apache License, Version 2.0 or the MIT license, at "
                                              b"your option."), set())
        self.assertEqual(licences.licences_in(b"../LICENSE-APACHE"), set())


if __name__ == "__main__":
    unittest.main()
