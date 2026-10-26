"""The release site that site.py writes (ADR 0727)."""
import base64
import contextlib
import hashlib
import importlib.util
import io
import pathlib
import re
import subprocess
import sys
import tempfile
import unittest

# The name site is the module of the standard library that Python loads at its start, so the file is loaded by path.
_specification = importlib.util.spec_from_file_location("release_site", pathlib.Path(__file__).with_name("site.py"))
site = importlib.util.module_from_spec(_specification)
sys.modules[_specification.name] = site
_specification.loader.exec_module(site)

BASE = "https://releases.example.test/junto/"
SERVER_INSTALL = "https://docs.example.test/junto/install.html"
TARGETS = ["macos-bundle-aarch64", "linux-directory-x86_64", "linux-appimage-x86_64", "windows-directory-x86_64"]


def write(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)


def release_directory(root, version, targets, first_install=True):
    directory = root / version
    directory.mkdir(parents=True)
    for target in targets:
        write(directory / f"junto-{version}-{target}.junto", f"archive {version} {target}".encode() * 3)
        write(directory / f"release-{version}-{target}.json", f'{{"release": "{version} {target}"}}'.encode())
        write(directory / f"latest-{target}.json", f'{{"latest": "{version} {target}"}}'.encode())
    if first_install:
        for index, name in enumerate([*site.first_install_names(version), site.SYMBOLS.format(v=version)]):
            write(directory / name, f"first install {name} ".encode() * (index + 1))
    return directory


class Fixture:
    def __init__(self, root):
        self.root = root
        self.releases = root / "releases"
        self.notes = root / "notes"
        self.output = root / "site"
        release_directory(self.releases, "0.1.0", TARGETS)
        release_directory(self.releases, "0.1.1", TARGETS[1:])
        (self.releases / "0.1.1-test.1").mkdir()
        (self.releases / "work").mkdir()
        write(self.notes / "0.1.0.md", b"# Junto 0.1.0\n\nThe first release.\n")
        write(self.notes / "0.1.1.md", "# Junto 0.1.1\n\n- A correction.\n".encode())

    def build(self):
        with contextlib.redirect_stdout(io.StringIO()) as printed:
            code = site.main([str(self.releases), str(self.notes), str(self.output),
                              "--base", BASE, "--server-install", SERVER_INSTALL])
        return code, printed.getvalue()

    def refused(self, test, pattern):
        with contextlib.redirect_stderr(io.StringIO()) as errors:
            code, _ = self.build()
        test.assertEqual(code, 1)
        test.assertRegex(errors.getvalue(), pattern)
        test.assertEqual([path.name for path in self.root.iterdir() if path.name.startswith((".site", "site"))], [],
                         "a failure leaves no output and no temporary directory")


def inline_elements(text, tag):
    return re.findall(rf"<{tag}>(.*?)</{tag}>", text, re.S)


def source_hash(text):
    return "'sha256-" + base64.b64encode(hashlib.sha256(text.encode()).digest()).decode() + "'"


class SiteTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.fixture = Fixture(pathlib.Path(directory.name))

    def built(self):
        code, printed = self.fixture.build()
        self.assertEqual(code, 0)
        return self.fixture.output, printed

    def test_the_index_offers_the_files_of_the_newest_version_with_their_size_and_sha256(self):
        output, _ = self.built()
        index = (output / "index.html").read_text()
        self.assertIn("<h1>Junto 0.1.1</h1>", index)
        for name in [*site.first_install_names("0.1.1"), site.SYMBOLS.format(v="0.1.1")]:
            data = (self.fixture.releases / "0.1.1" / name).read_bytes()
            with self.subTest(name):
                self.assertIn(f'href="download/0.1.1/{name}"', index)
                self.assertIn(hashlib.sha256(data).hexdigest(), index)
                self.assertIn(f"{len(data):,} bytes", index)
                self.assertEqual((output / "download/0.1.1" / name).read_bytes(), data)
        self.assertNotIn("download/0.1.0/", index)
        self.assertIn('href="notes/0.1.1.html"', index)
        self.assertIn('href="notes/0.1.0.html#files"', index)
        self.assertIn(f'href="{SERVER_INSTALL}"', index)
        self.assertIn("Open Anyway", index)
        self.assertIn("Run anyway", index)
        self.assertIn("Smart App Control", index)
        self.assertIn("sudo apt install ./junto_0.1.1_amd64.deb", index)
        self.assertIn("--appimage-extract-and-run", index)
        self.assertIn("shasum -a 256", index)
        self.assertIn("Get-FileHash -Algorithm SHA256", index)
        self.assertIn("sha256sum -c", index)
        self.assertIn(f"{BASE}download/0.1.1/SHA256SUMS", index)
        self.assertLess(index.index('id="system-macos"'), index.index('id="system-windows"'))
        self.assertLess(index.index('id="system-windows"'), index.index('id="system-linux"'))

    def test_sha256sums_verifies_against_the_copied_files_of_each_version(self):
        output, _ = self.built()
        for version in ["0.1.0", "0.1.1"]:
            directory = output / "download" / version
            lines = (directory / "SHA256SUMS").read_text().splitlines()
            names = [line.split("  ", 1)[1] for line in lines]
            self.assertEqual(names, sorted(path.name for path in directory.iterdir() if path.name != "SHA256SUMS"))
            for line in lines:
                digest, name = line.split("  ", 1)
                self.assertRegex(digest, "^[0-9a-f]{64}$")
                self.assertEqual(hashlib.sha256((directory / name).read_bytes()).hexdigest(), digest)
            result = subprocess.run(["sha256sum", "-c", "SHA256SUMS"], cwd=directory, capture_output=True, text=True)
            if result.returncode != 127:
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_the_update_source_holds_every_version_and_the_newest_latest_file_of_each_target(self):
        output, printed = self.built()
        updates = output / "updates"
        expected = set()
        for version, targets in [("0.1.0", TARGETS), ("0.1.1", TARGETS[1:])]:
            for target in targets:
                for name in [f"junto-{version}-{target}.junto", f"release-{version}-{target}.json"]:
                    expected.add(name)
                    self.assertEqual((updates / name).read_bytes(), (self.fixture.releases / version / name).read_bytes())
        for target in TARGETS[1:]:
            self.assertEqual((updates / f"latest-{target}.json").read_bytes(),
                             (self.fixture.releases / "0.1.1" / f"latest-{target}.json").read_bytes())
        self.assertEqual((updates / f"latest-{TARGETS[0]}.json").read_bytes(),
                         (self.fixture.releases / "0.1.0" / f"latest-{TARGETS[0]}.json").read_bytes())
        expected |= {f"latest-{target}.json" for target in TARGETS}
        self.assertEqual({path.name for path in updates.iterdir()}, expected)
        self.assertFalse((output / "download/0.1.1-test.1").exists())
        self.assertFalse((output / "notes/work.html").exists())
        lines = printed.splitlines()
        self.assertIn(f"Update source of the clients (JUNTO_UPDATE_SOURCE): {BASE}updates/", lines)
        self.assertEqual(lines[-3:], site.upload_order())
        self.assertTrue(any(line.endswith("  index.html") for line in lines))

    def test_the_command_does_not_change_the_releases_directory(self):
        before = {path: path.read_bytes() for path in self.fixture.releases.rglob("*") if path.is_file()}
        self.built()
        after = {path: path.read_bytes() for path in self.fixture.releases.rglob("*") if path.is_file()}
        self.assertEqual(before, after)

    def test_an_older_version_shows_the_files_it_holds(self):
        (self.fixture.releases / "0.1.0" / "Junto-0.1.0-macos-aarch64.dmg").unlink()
        (self.fixture.releases / "0.1.0" / "junto_0.1.0_amd64.deb").unlink()
        output, _ = self.built()
        notes = (output / "notes/0.1.0.html").read_text()
        self.assertIn('href="../download/0.1.0/Junto-0.1.0-macos-aarch64.zip"', notes)
        self.assertNotIn("Junto-0.1.0-macos-aarch64.dmg", notes)
        self.assertFalse((output / "download/0.1.0/junto_0.1.0_amd64.deb").exists())
        self.assertNotIn("junto_0.1.0_amd64.deb", (output / "download/0.1.0/SHA256SUMS").read_text())

    def test_the_notes_render_and_escape_and_refuse_other_schemes(self):
        write(self.fixture.notes / "0.1.1.md", "\n".join([
            "# Junto 0.1.1",
            "## Changes",
            "### Calls",
            "A paragraph with `code <b>`, **bold** and <script>alert(1)</script>.",
            "- [A link](https://example.test/a?b=1&c=2)",
            "- [Relative](../install.html)",
            "- [Bad](javascript:alert(1))",
            "- [Bad two](data:text/html,x) and [Other host](//example.test/x)",
            "```sh",
            "echo <tag> & done",
            "```",
            "",
        ]).encode())
        output, _ = self.built()
        notes = (output / "notes/0.1.1.html").read_text()
        self.assertIn("<h1>Junto 0.1.1</h1>", notes)
        self.assertIn("<h2>Changes</h2>", notes)
        self.assertIn("<h3>Calls</h3>", notes)
        self.assertIn("<code>code &lt;b&gt;</code>", notes)
        self.assertIn("<strong>bold</strong>", notes)
        self.assertIn("&lt;script&gt;alert(1)&lt;/script&gt;", notes)
        self.assertEqual(inline_elements(notes, "script"), [])
        self.assertIn('<li><a href="https://example.test/a?b=1&amp;c=2">A link</a></li>', notes)
        self.assertIn('<a href="../install.html">Relative</a>', notes)
        self.assertNotIn('href="javascript:', notes)
        self.assertNotIn('href="data:', notes)
        self.assertNotIn('href="//', notes)
        self.assertIn("[Bad](javascript:alert(1))", notes)
        self.assertIn("<pre><code>echo &lt;tag&gt; &amp; done</code></pre>", notes)

    def test_the_markdown_renderer_refuses_links_with_control_characters(self):
        self.assertFalse(site.link_allowed("\x01javascript:alert(1)"))
        self.assertFalse(site.link_allowed("JavaScript:alert(1)"))
        self.assertFalse(site.link_allowed("http://example.test/"))
        self.assertTrue(site.link_allowed("https://example.test/"))
        self.assertTrue(site.link_allowed("install.html#upgrade"))

    def test_the_pages_load_nothing_from_another_place_and_the_policy_names_their_inline_sources(self):
        output, _ = self.built()
        pages = sorted(output.rglob("*.html"))
        self.assertEqual([path.relative_to(output).as_posix() for path in pages],
                         ["index.html", "notes/0.1.0.html", "notes/0.1.1.html"])
        for path in pages:
            text = path.read_text()
            with self.subTest(path.name):
                self.assertTrue(text.startswith('<!doctype html>\n<html lang="en">'))
                self.assertIn('<meta charset="utf-8">', text)
                self.assertIn('<meta name="viewport" content="width=device-width, initial-scale=1">', text)
                for forbidden in ["<script src", "@import", "url(", "<link", "<img", "<iframe", "@font-face"]:
                    self.assertNotIn(forbidden, text)
                self.assertNotRegex(text, r"\bsrc=")
                self.assertNotRegex(text, r"\bstyle=")
                policy = re.search(r'<meta http-equiv="Content-Security-Policy" content="([^"]*)">', text)[1]
                directives = dict(part.strip().split(" ", 1) for part in policy.split(";"))
                self.assertEqual(directives["default-src"], "'self'")
                styles = inline_elements(text, "style")
                self.assertEqual(len(styles), 1)
                self.assertEqual(directives["style-src"], source_hash(styles[0]))
                scripts = inline_elements(text, "script")
                if path.name == "index.html":
                    self.assertEqual(len(scripts), 1)
                    self.assertEqual(directives["script-src"], source_hash(scripts[0]))
                    self.assertIn("navigator.userAgentData", scripts[0])
                    self.assertIn("navigator.platform", scripts[0])
                    self.assertIn("navigator.userAgent", scripts[0])
                    self.assertIn('getElementById("system-"', scripts[0])
                else:
                    self.assertEqual(scripts, [])
                    self.assertEqual(directives["script-src"], "'none'")

    def test_the_command_refuses_an_existing_output_directory(self):
        self.fixture.output.mkdir()
        with contextlib.redirect_stderr(io.StringIO()) as errors:
            code, _ = self.fixture.build()
        self.assertEqual(code, 1)
        self.assertIn("the output directory exists", errors.getvalue())
        self.assertEqual(list(self.fixture.output.iterdir()), [])

    def test_the_command_refuses_a_version_without_notes(self):
        (self.fixture.notes / "0.1.0.md").unlink()
        self.fixture.refused(self, r"missing notes: .*0\.1\.0\.md")

    def test_the_command_names_each_missing_file_of_the_newest_version(self):
        (self.fixture.releases / "0.1.1" / "Junto-0.1.1-windows-x86_64-setup.exe").unlink()
        (self.fixture.releases / "0.1.1" / "junto_0.1.1_amd64.deb").unlink()
        self.fixture.refused(self, r"(?s)Junto-0\.1\.1-windows-x86_64-setup\.exe.*junto_0\.1\.1_amd64\.deb")

    def test_the_command_refuses_a_newest_version_without_a_latest_file(self):
        for target in TARGETS[1:]:
            (self.fixture.releases / "0.1.1" / f"latest-{target}.json").unlink()
        self.fixture.refused(self, r"latest-<target>\.json")

    def test_the_command_refuses_a_latest_file_without_its_archive(self):
        (self.fixture.releases / "0.1.0" / "junto-0.1.0-macos-bundle-aarch64.junto").unlink()
        self.fixture.refused(self, r"missing file: .*junto-0\.1\.0-macos-bundle-aarch64\.junto")

    def test_the_command_refuses_an_unknown_file(self):
        for name in ["notes.txt", "junto-0.1.1-linux-bundle-x86_64.junto", "release-0.1.0-linux-appimage-x86_64.json"]:
            with self.subTest(name):
                write(self.fixture.releases / "0.1.1" / name, b"x")
                self.fixture.refused(self, rf"unknown file: .*{re.escape(name)}")
                (self.fixture.releases / "0.1.1" / name).unlink()

    def test_the_command_refuses_an_address_that_is_not_https_or_does_not_end_in_a_slash(self):
        for base in ["http://releases.example.test/", "https://releases.example.test/junto",
                     "https://user@releases.example.test/", "https://releases.example.test/?a=1"]:
            with self.subTest(base), self.assertRaises(site.SiteError):
                site.build(self.fixture.releases, self.fixture.notes, self.fixture.output, base, SERVER_INSTALL)
        self.assertFalse(self.fixture.output.exists())

    def test_a_failure_while_writing_leaves_no_output_directory(self):
        original = site.write_site

        def failing(*arguments):
            original(*arguments)
            raise OSError("no space left on the device")

        site.write_site = failing
        self.addCleanup(setattr, site, "write_site", original)
        self.fixture.refused(self, "no space left on the device")


if __name__ == "__main__":
    unittest.main()
