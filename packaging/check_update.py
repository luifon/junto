#!/usr/bin/env python3
"""Real signing, update and documented archive recovery of macOS test bundles; no GUI is started.

Builds junto-release and junto-recover with test keys and a test certificate pin in the target
directory of the repository; a local caller holds tools/serial.sh."""
import os
import pathlib
import plistlib
import shutil
import subprocess
import sys
import tempfile

from bundle import assemble
from check_signing import requirement
from configuration import architecture
from create_identity import create, run
from keychains import cleanup, signals
from programs import outcome, program
import release_tools


def check():
    arch = architecture()
    before = run("security", "list-keychains", "-d", "user")
    default_before = run("security", "default-keychain", "-d", "user")
    with signals(), tempfile.TemporaryDirectory(prefix="junto-recovery-") as temporary:
        root = pathlib.Path(temporary)
        keychain = root / "identity/signing.keychain-db"
        try:
            release_tools.run("cargo", "build", "--locked", "-p", "junto-client-core", "--bin", "junto-release",
                              cwd=release_tools.REPOSITORY, timeout=3600)
            files = release_tools.keys(pathlib.Path(os.environ.get("CARGO_TARGET_DIR", release_tools.REPOSITORY / "target")) / "debug/junto-release", root)
            identity = create(root / "identity")
            pin = root / "certificate.sha1"
            pin.write_text(identity)
            tool, recover = release_tools.build(files, pin)
            environment = os.environ.copy()
            environment.update(JUNTO_CODESIGN_IDENTITY=identity, JUNTO_CODESIGN_KEYCHAIN=str(keychain),
                               JUNTO_TEST_CERTIFICATE_FILE=str(pin))
            succeeding, failing = program(root / "exit-0", 0), program(root / "exit-1", 1)
            installed = assemble(succeeding, root / "installed/Junto.app", "1.0.0", recover)
            candidate = assemble(failing, root / "source-2/Junto.app", "2.0.0", recover)
            first = assemble(succeeding, root / "source-1/Junto.app", "1.0.0", recover)

            def ran(arguments):
                return subprocess.run([str(argument) for argument in arguments], capture_output=True, text=True,
                                      timeout=60)

            def sign(bundle, env=environment):
                subprocess.run([sys.executable, str(pathlib.Path(__file__).with_name("sign.py")), str(bundle)], env=env,
                               check=True, timeout=60)

            def package(bundle, output, version):
                release_tools.package(tool, files, bundle.parent, output, version, "macos", "bundle", arch)

            def version(bundle):
                return plistlib.loads((bundle / "Contents/Info.plist").read_bytes())["CFBundleShortVersionString"]

            for bundle in [installed, candidate, first]:
                sign(bundle)
            expected = requirement(installed)
            release = root / "release"
            package(candidate, release, "2.0.0")
            # The source keeps the release of every version; the installation holds no record.
            package(first, root / "release-1", "1.0.0")
            for file in (root / "release-1").iterdir():
                if not file.name.startswith("latest-"):
                    shutil.copyfile(file, release / file.name)
            with release_tools.source(release) as (server, address):
                update = [tool, "update", address, installed]
                installed.parent.chmod(0o555)
                try:
                    result = ran(update)
                finally:
                    installed.parent.chmod(0o755)
                assert result.returncode != 0 and "move Junto" in result.stderr, outcome(result)
                assert server.paths == [], server.paths
                result = ran(update)
                assert result.returncode == 0, outcome(result)
                assert "version 1.0.0 is kept for a rollback" in result.stdout, outcome(result)
                assert server.paths == [f"/latest-macos-bundle-{arch}.json", f"/junto-2.0.0-macos-bundle-{arch}.junto",
                                        f"/release-1.0.0-macos-bundle-{arch}.json",
                                        f"/junto-1.0.0-macos-bundle-{arch}.junto"], server.paths
                record = installed.parent / ".Junto.app.junto-update/record"
                assert version(installed) == "2.0.0"
                assert requirement(installed) == expected
                result = ran([installed / "Contents/MacOS/junto"])
                assert result.returncode == 1, outcome(result)
                assert (record / "previous/archive.junto").is_file()
                assert not list((installed.parent / ".Junto.app.junto-update").rglob("*.app"))
                # This is the documented recovery command; the new application is never run.
                recovery = [record / "previous/recover", "rollback", installed]
                backup = (record / "previous/archive.junto").read_bytes()
                (record / "previous/archive.junto").unlink()
                (record / "previous/archive.junto").write_bytes(b"changed archive")
                result = ran(recovery)
                assert result.returncode != 0 and version(installed) == "2.0.0", outcome(result)
                (record / "previous/archive.junto").unlink()
                (record / "previous/archive.junto").write_bytes(backup)
                result = ran(recovery)
                assert result.returncode == 0, outcome(result)
                assert version(installed) == "1.0.0"
                assert requirement(installed) == expected
                result = ran([installed / "Contents/MacOS/junto"])
                assert result.returncode == 0, outcome(result)
                # The record now names the restored release as installed and has no older one;
                # the recovery program, kept with it, refuses a second rollback.
                assert not (record / "previous").exists()
                result = ran([record / "installed/recover", "rollback", installed])
                assert result.returncode != 0 and version(installed) == "1.0.0", outcome(result)
                for identity_kind in ["adhoc", "other"]:
                    bad = assemble(succeeding, root / identity_kind / "Junto.app", "3.0.0", recover)
                    if identity_kind == "adhoc":
                        run("/usr/bin/codesign", "--force", "--sign", "-", str(bad))
                    else:
                        other_identity = create(root / "other-identity")
                        other_pin = root / "other-certificate.sha1"
                        other_pin.write_text(other_identity)
                        other_environment = environment.copy()
                        other_environment.update(JUNTO_CODESIGN_IDENTITY=other_identity,
                                                 JUNTO_CODESIGN_KEYCHAIN=str(root / "other-identity/signing.keychain-db"),
                                                 JUNTO_TEST_CERTIFICATE_FILE=str(other_pin))
                        sign(bad, other_environment)
                    bad_release = root / (identity_kind + "-release")
                    package(bad, bad_release, "3.0.0")
                    for file in bad_release.iterdir():
                        shutil.copyfile(file, release / file.name)
                    result = ran(update)
                    assert result.returncode != 0, outcome(result)
                    assert version(installed) == "1.0.0"
            print("20 update and archive-recovery assertions passed", flush=True)
        finally:
            cleanup([keychain, root / "other-identity/signing.keychain-db"], before)
            assert run("security", "list-keychains", "-d", "user") == before
            assert run("security", "default-keychain", "-d", "user") == default_before
            print("Recovery search list before:\n" + before + "Recovery search list after:\n"
                  + run("security", "list-keychains", "-d", "user"), flush=True)


if __name__ == "__main__":
    if len(sys.argv) != 1:
        sys.exit("usage: packaging/check_update.py")
    check()
