"""The files of one GitHub release that release_assets.py collects (ADR 0727), and the renewal of
their freshness by renew_freshness.py (ADR 0724)."""
import base64
import contextlib
import hashlib
import io
import json
import pathlib
import shutil
import subprocess
import tempfile
import unittest
import unittest.mock

import release_assets
import renew_freshness

VERSION = "0.4.1"
DAY = 24 * 60 * 60
NOW = 1_800_000_000


def envelope(version, target, size, expires):
    system, package, architecture = target.split("-")
    release = {"format": 1, "version": version, "system": system, "package": package, "architecture": architecture,
               "archive": f"junto-{version}-{target}.junto", "size": size, "sha256": "00" * 32,
               "archive_signatures": []}
    freshness = {"format": 1, "version": version, "release_sha256": "00" * 32, "expires": expires}
    encode = lambda value: base64.b64encode(json.dumps(value).encode()).decode()
    return json.dumps({"payload": encode(release), "signatures": [],
                       "freshness": {"payload": encode(freshness), "signatures": []}}).encode()


def release_directory(directory, version=VERSION, expires=NOW + 2 * DAY):
    directory.mkdir(parents=True)
    for index, template in enumerate([*release_assets.FIRST_INSTALL, release_assets.SYMBOLS]):
        name = template.format(v=version)
        (directory / name).write_bytes(f"first install {name} ".encode() * (index + 1))
    for target in release_assets.TARGETS:
        archive = f"archive {version} {target}".encode() * 3
        (directory / release_assets.archive_name(version, target)).write_bytes(archive)
        information = envelope(version, target, len(archive), expires)
        (directory / release_assets.release_name(version, target)).write_bytes(information)
        (directory / release_assets.latest_name(target)).write_bytes(information)
    return directory


class AssetTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = pathlib.Path(temporary.name)
        self.release = release_directory(self.root / "release")
        self.output = self.root / "assets"
        notes = tempfile.TemporaryDirectory()
        self.addCleanup(notes.cleanup)
        self.notes = pathlib.Path(notes.name)
        (self.notes / f"{VERSION}.md").write_text(f"# Junto {VERSION}\n\n## What is new\n\n- A change\n")
        patched = unittest.mock.patch.object(release_assets, "RELEASES", self.notes)
        patched.start()
        self.addCleanup(patched.stop)

    def test_a_version_without_the_notes_that_the_client_carries_is_refused(self):
        (self.notes / f"{VERSION}.md").write_text(f"# Junto {VERSION}\n\n## Packages\n")
        self.refused(r"docs/releases/0\.4\.1\.md with the section '## What is new'")
        (self.notes / f"{VERSION}.md").unlink()
        self.refused(r"is committed before the first build of the version")
        code, _, errors = self.collect("--test", "1", "--notes", str(self.root / "notes.md"))
        self.assertEqual(code, 1, errors)
        # A section without text is no notes either.
        (self.notes / f"{VERSION}.md").write_text(f"# Junto {VERSION}\n\n## What is new\n\n## Packages\n- a\n")
        self.refused(r"is committed before the first build of the version")

    def test_the_section_ends_at_the_next_heading_of_its_level_as_the_client_reads_it(self):
        self.assertEqual(release_assets.what_is_new("# J\n\n## What is new\n\n- a\n\n### b\n- c\n## Files\n- d\n"),
                         "- a\n\n### b\n- c")
        self.assertIsNone(release_assets.what_is_new("## What is new\n\n# Junto\n"))
        self.assertIsNone(release_assets.what_is_new("## Contents\n- a\n"))

    def test_the_notes_of_0_1_0_published_before_the_section_are_accepted(self):
        release = release_directory(self.root / "release-0.1.0", version="0.1.0")
        (self.notes / "0.1.0.md").write_text("# Junto 0.1.0\n\n## Contents\n\n- Chat\n")
        for extra in ([], ["--test", "1", "--notes", str(self.notes / "0.1.0.md")]):
            output = self.root / f"assets-{len(extra)}"
            with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()) as errors:
                code = release_assets.main(["0.1.0", str(release), str(output), *extra])
            self.assertEqual(code, 0, errors.getvalue())

    def collect(self, *extra):
        with contextlib.redirect_stdout(io.StringIO()) as printed, contextlib.redirect_stderr(io.StringIO()) as errors:
            code = release_assets.main([VERSION, str(self.release), str(self.output), *extra])
        return code, printed.getvalue(), errors.getvalue()

    def refused(self, pattern):
        code, _, errors = self.collect()
        self.assertEqual(code, 1)
        self.assertRegex(errors, pattern)
        self.assertEqual([path.name for path in self.root.iterdir() if path.name not in ("release",)], [],
                         "a refusal leaves no output and no temporary directory")

    def test_every_file_is_copied_and_sha256sums_holds_all_but_the_latest_files(self):
        code, printed, _ = self.collect()
        self.assertEqual(code, 0)
        names = release_assets.expected_names(VERSION)
        self.assertEqual(sorted(path.name for path in self.output.iterdir()), sorted([*names, "SHA256SUMS"]))
        for name in names:
            self.assertEqual((self.output / name).read_bytes(), (self.release / name).read_bytes(), name)
        lines = (self.output / "SHA256SUMS").read_text().splitlines()
        listed = [line.split("  ", 1)[1] for line in lines]
        self.assertEqual(listed, sorted(name for name in names if not name.startswith("latest-")))
        for line in lines:
            digest, name = line.split("  ", 1)
            self.assertEqual(hashlib.sha256((self.output / name).read_bytes()).hexdigest(), digest)
        if shutil.which("sha256sum"):
            subprocess.run(["sha256sum", "--quiet", "-c", "SHA256SUMS"], cwd=self.output, check=True)
        self.assertIn(f"gh release create v{VERSION} --repo luifon/junto --draft --verify-tag --title 'Junto {VERSION}'",
                      printed)
        self.assertIn(f"gh release edit v{VERSION} --repo luifon/junto --draft=false --latest", printed)
        self.assertIn(f"--notes-file docs/releases/{VERSION}.md", printed)
        self.assertIn("JUNTO_UPDATE_SOURCE): https://github.com/luifon/junto/releases/", printed)
        self.assertNotIn("--prerelease", printed)
        self.assertNotIn("-test.", printed)
        self.assertNotIn("git tag", printed)
        self.assertEqual(sorted(path.name for path in self.release.iterdir()), sorted(names),
                         "the release directory is not changed")

    def test_a_test_build_holds_only_the_first_install_files_under_a_tag_and_title_of_its_own(self):
        code, printed, _ = self.collect("--test", "4", "--notes", "/private/notes.md")
        self.assertEqual(code, 0)
        names = release_assets.first_install_names(VERSION)
        self.assertEqual(len(names), 8)
        self.assertEqual(sorted(path.name for path in self.output.iterdir()), sorted([*names, "SHA256SUMS"]))
        lines = (self.output / "SHA256SUMS").read_text().splitlines()
        self.assertEqual([line.split("  ", 1)[1] for line in lines], sorted(names))
        for line in lines:
            digest, name = line.split("  ", 1)
            self.assertEqual(hashlib.sha256((self.release / name).read_bytes()).hexdigest(), digest)
        self.assertIn(f"git tag v{VERSION}-test.4 <release commit>\n", printed)
        self.assertIn(f"gh release create v{VERSION}-test.4 --repo luifon/junto --draft --verify-tag "
                      f"--title 'Junto {VERSION} test 4' --notes-file /private/notes.md {self.output}/*\n", printed)
        self.assertIn(f"gh release edit v{VERSION}-test.4 --repo luifon/junto --draft=false --prerelease\n", printed)
        self.assertNotIn("--latest", printed)
        self.assertNotIn(f"v{VERSION} ", printed)
        self.assertEqual(release_assets.quoted("it's"), "'it'\\''s'")

    def test_a_test_build_needs_only_the_first_install_files_and_its_notes(self):
        for target in release_assets.TARGETS:
            for name in [release_assets.latest_name(target), release_assets.release_name(VERSION, target),
                         release_assets.archive_name(VERSION, target)]:
                (self.release / name).unlink()
        code, _, errors = self.collect("--test", "1")
        self.assertEqual(code, 1)
        self.assertIn("a test build needs --notes", errors)
        self.assertFalse(self.output.exists())
        code, printed, _ = self.collect("--test", "1", "--notes", "notes.md")
        self.assertEqual(code, 0)
        self.assertIn(f"--title 'Junto {VERSION} test 1'", printed)
        self.assertEqual(sorted(path.name for path in self.output.iterdir()),
                         sorted([*release_assets.first_install_names(VERSION), "SHA256SUMS"]))

    def test_a_test_build_refuses_a_wrong_number_and_a_missing_or_unknown_file(self):
        for number in ["0", "04", "-1", "4a", " 4"]:
            with self.subTest(number):
                code, _, errors = self.collect("--test", number, "--notes", "notes.md")
                self.assertEqual(code, 1)
                self.assertIn("not a test number", errors)
                self.assertFalse(self.output.exists())
        dmg = self.release / f"Junto-{VERSION}-macos-aarch64.dmg"
        dmg.unlink()
        (self.release / "notes.txt").write_bytes(b"other")
        code, _, errors = self.collect("--test", "4", "--notes", "notes.md")
        self.assertEqual(code, 1)
        self.assertRegex(errors, f"missing file: .*{dmg.name}")
        self.assertRegex(errors, "unknown file: .*notes.txt")
        self.assertEqual([path.name for path in self.root.iterdir()], ["release"])

    def test_a_missing_file_is_named(self):
        for name in [f"Junto-{VERSION}-macos-aarch64.dmg", f"Junto-{VERSION}-windows-x86_64.pdb",
                     "latest-linux-appimage-x86_64.json", f"junto-{VERSION}-windows-directory-x86_64.junto"]:
            with self.subTest(name):
                data = (self.release / name).read_bytes()
                (self.release / name).unlink()
                self.refused(f"missing file: .*{name}")
                (self.release / name).write_bytes(data)

    def test_an_unknown_file_or_a_file_of_another_version_is_refused(self):
        for name in ["notes.txt", "junto-0.4.0-linux-directory-x86_64.junto", ".DS_Store"]:
            with self.subTest(name):
                (self.release / name).write_bytes(b"other")
                self.refused(f"unknown file: .*{name}")
                (self.release / name).unlink()

    def test_update_files_that_do_not_belong_together_are_refused(self):
        latest = self.release / "latest-macos-bundle-aarch64.json"
        original = latest.read_bytes()
        latest.write_bytes(envelope(VERSION, "macos-bundle-aarch64", 1, NOW + 3 * DAY))
        self.refused("latest-macos-bundle-aarch64.json holds another signed release than "
                     "release-0.4.1-macos-bundle-aarch64.json")
        renewed = json.loads(original)
        renewed["freshness"]["payload"] = base64.b64encode(b'{"expires": 1}').decode()
        latest.write_bytes(json.dumps(renewed).encode())
        code, _, _ = self.collect()
        self.assertEqual(code, 0, "a renewed latest file belongs to its release")
        shutil.rmtree(self.output)
        latest.write_bytes(original)
        for wrong, pattern in [(envelope("0.4.0", "macos-bundle-aarch64", 1, NOW), "another archive, version"),
                               (envelope(VERSION, "linux-directory-x86_64", 1, NOW), "another architecture, archive, package, system"),
                               (envelope(VERSION, "macos-bundle-aarch64", 1, NOW), "another size")]:
            with self.subTest(pattern):
                for name in [latest.name, f"release-{VERSION}-macos-bundle-aarch64.json"]:
                    (self.release / name).write_bytes(wrong)
                self.refused(pattern)
        for name in [latest.name, f"release-{VERSION}-macos-bundle-aarch64.json"]:
            (self.release / name).write_bytes(b"not JSON")
        self.refused("not release information")

    def test_an_existing_output_and_a_version_that_is_not_three_numbers_are_refused(self):
        self.output.mkdir()
        code, _, errors = self.collect()
        self.assertEqual(code, 1)
        self.assertIn("the output directory exists", errors)
        self.output.rmdir()
        with contextlib.redirect_stderr(io.StringIO()):
            code = release_assets.main(["0.4", str(self.release), str(self.output)])
        self.assertEqual(code, 1)
        self.assertFalse(self.output.exists())


class Completed:
    def __init__(self, returncode=0, stderr=""):
        self.returncode, self.stderr, self.stdout = returncode, stderr, ""


class RenewalTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = pathlib.Path(temporary.name)
        self.retained = release_directory(self.root / "release")
        self.commands = []

    def refresh(self, days=30, fail=False):
        """A release tool whose refresh writes a new freshness end into the latest file."""
        def run(command, **_options):
            self.commands.append(command)
            if fail:
                return Completed(1, "invalid release: untrusted signature")
            latest = pathlib.Path(command[2])
            data = json.loads(latest.read_bytes())
            freshness = json.loads(base64.b64decode(data["freshness"]["payload"]))
            freshness["expires"] = NOW + days * DAY
            data["freshness"]["payload"] = base64.b64encode(json.dumps(freshness).encode()).decode()
            latest.write_bytes(json.dumps(data).encode())
            return Completed()
        return run

    def test_each_latest_file_is_refreshed_from_the_retained_release_and_its_archive(self):
        renewed = renew_freshness.renew("/tools/junto-release", self.retained, ["/keys/a.seed", "/keys/b.seed"],
                                        run=self.refresh(), now=lambda: NOW)
        self.assertEqual([(path.name, version, ends) for path, version, ends in renewed],
                         [(f"latest-{target}.json", VERSION, NOW + 30 * DAY) for target in release_assets.TARGETS])
        self.assertEqual(self.commands, [
            ["/tools/junto-release", "refresh", str(self.retained / f"latest-{target}.json"),
             str(self.retained / f"junto-{VERSION}-{target}.junto"), VERSION, "/keys/a.seed", "/keys/b.seed"]
            for target in release_assets.TARGETS])
        for target in release_assets.TARGETS:
            self.assertEqual(renew_freshness.expiry(self.retained / f"release-{VERSION}-{target}.json"), NOW + 2 * DAY,
                             "the release file of the version is not changed")

    def test_a_failed_refresh_or_a_short_validity_stops_the_renewal(self):
        with self.assertRaisesRegex(renew_freshness.RenewalError, "untrusted signature"):
            renew_freshness.renew("tool", self.retained, ["key"], run=self.refresh(fail=True), now=lambda: NOW)
        self.assertEqual(len(self.commands), 1)
        with self.assertRaisesRegex(renew_freshness.RenewalError, "fresh only until"):
            renew_freshness.renew("tool", self.retained, ["key"], run=self.refresh(days=10), now=lambda: NOW)

    def test_a_directory_with_two_versions_or_without_a_latest_file_is_refused(self):
        other = self.retained / "release-0.4.0-linux-directory-x86_64.json"
        other.write_bytes(b"{}")
        with self.assertRaisesRegex(renew_freshness.RenewalError, "holds 2 release files of linux-directory-x86_64"):
            renew_freshness.renew("tool", self.retained, ["key"], run=self.refresh(), now=lambda: NOW)
        other.unlink()
        (self.retained / "latest-windows-directory-x86_64.json").unlink()
        with self.assertRaisesRegex(renew_freshness.RenewalError, "missing file: .*latest-windows-directory"):
            renew_freshness.renew("tool", self.retained, ["key"], run=self.refresh(), now=lambda: NOW)
        self.assertEqual(self.commands, [], "nothing is refreshed before every file is found")

    def test_an_upload_needs_a_tag_of_the_version_and_a_signed_in_github_command(self):
        signed_in = lambda command, **_: self.commands.append(command) or Completed()
        renew_freshness.check_upload(f"v{VERSION}", self.retained, run=signed_in)
        self.assertEqual(self.commands, [["gh", "auth", "status"]])
        for tag in ["v0.4.0", "v0.4.10", f"v{VERSION}-test.4", f"{VERSION}", "latest"]:
            with self.assertRaisesRegex(renew_freshness.RenewalError, f"does not name version {VERSION}"):
                renew_freshness.check_upload(tag, self.retained, run=signed_in)
        with self.assertRaisesRegex(renew_freshness.RenewalError, "not signed in: no token"):
            renew_freshness.check_upload(f"v{VERSION}", self.retained, run=lambda *_, **__: Completed(1, "no token"))

    def test_the_upload_replaces_the_latest_files_and_reads_them_back_from_the_newest_release(self):
        files = [self.retained / f"latest-{target}.json" for target in release_assets.TARGETS]
        asked = []

        def served(address):
            asked.append(address)
            return (self.retained / address.rsplit("/", 1)[1]).read_bytes()

        renew_freshness.upload(f"v{VERSION}", files, run=lambda command, **_: self.commands.append(command)
                               or Completed(), fetch=served)
        self.assertEqual(self.commands, [["gh", "release", "upload", f"v{VERSION}", *map(str, files), "--repo",
                                          "luifon/junto", "--clobber"]])
        self.assertEqual(asked, [f"https://github.com/luifon/junto/releases/latest/download/{path.name}"
                                 for path in files])
        with self.assertRaisesRegex(renew_freshness.RenewalError, "does not serve the renewed latest-macos"):
            renew_freshness.upload("v0.4.0", files[:1], run=lambda *_, **__: Completed(), fetch=lambda _: b"old")
        with self.assertRaisesRegex(renew_freshness.RenewalError, "upload to v0.4.1 failed: no release"):
            renew_freshness.upload(f"v{VERSION}", files, run=lambda *_, **__: Completed(1, "no release"),
                                   fetch=served)


if __name__ == "__main__":
    unittest.main()
