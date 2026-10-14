#!/usr/bin/env python3
"""Real signing, update and documented archive recovery; no GUI is started."""
import functools
import http.server
import os
import pathlib
import plistlib
import shutil
import subprocess
import sys
import tempfile
import threading
from bundle import assemble
from check_signing import requirement
from configuration import architecture
from create_identity import create, run
from keychains import cleanup, signals


def check(tool):
    tool = str(pathlib.Path(tool).absolute())
    arch = architecture()
    before = run("security", "list-keychains", "-d", "user")
    default_before = run("security", "default-keychain", "-d", "user")
    with signals(), tempfile.TemporaryDirectory(prefix="junto-recovery-") as temporary:
        root = pathlib.Path(temporary)
        secret, public = root / "release.seed", root / "release.json"
        fresh_secret, fresh_public = root / "freshness.seed", root / "freshness.json"
        for seed, keys in [(secret, public), (fresh_secret, fresh_public)]:
            run(tool, "keygen", str(seed), str(keys))
        keychain = root / "identity/signing.keychain-db"
        server = thread = None
        try:
            identity = create(root / "identity")
            pin = root / "certificate.sha1"
            pin.write_text(identity)
            environment = os.environ.copy()
            environment.update(JUNTO_CODESIGN_IDENTITY=identity, JUNTO_CODESIGN_KEYCHAIN=str(keychain),
                               JUNTO_TEST_CERTIFICATE_FILE=str(pin))
            installed = assemble("/usr/bin/true", root / "installed/Junto.app", "1.0.0", tool)
            candidate = assemble("/usr/bin/false", root / "source/Junto.app", "2.0.0", tool)
            def sign(bundle, env=environment):
                subprocess.run([sys.executable, str(pathlib.Path(__file__).with_name("sign.py")), str(bundle)], env=env, check=True, timeout=30)
            def package(bundle, output, version):
                run(tool, "package", str(bundle.parent), str(output), str(secret), str(fresh_secret), version, "macos", arch)
            def version(bundle):
                return plistlib.loads((bundle / "Contents/Info.plist").read_bytes())["CFBundleShortVersionString"]
            for bundle in [installed, candidate]:
                sign(bundle)
            expected = requirement(installed)
            initial = root / "release-v1"
            package(installed, initial, "1.0.0")
            receipts = installed.parent / ".Junto.releases"
            receipts.mkdir()
            shutil.copyfile(initial / f"latest-macos-{arch}.json", receipts / "1.0.0.json")
            release = root / "release"
            package(candidate, release, "2.0.0")
            class CountedHandler(http.server.SimpleHTTPRequestHandler):
                def do_GET(self):
                    self.server.requests += 1
                    super().do_GET()
            server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), functools.partial(CountedHandler, directory=str(release)))
            server.requests = 0
            thread = threading.Thread(target=server.serve_forever)
            thread.start()
            update = [tool, "update", f"http://127.0.0.1:{server.server_port}/", str(public), str(fresh_public), str(installed)]
            installed.parent.chmod(0o555)
            try:
                result = subprocess.run(update, capture_output=True, text=True, timeout=30)
            finally:
                installed.parent.chmod(0o755)
            assert result.returncode != 0 and "move Junto" in result.stderr
            assert server.requests == 0
            run(*update)
            previous = installed.parent / ".Junto.previous"
            assert version(installed) == "2.0.0"
            assert requirement(installed) == expected
            assert subprocess.run([str(installed / "Contents/MacOS/junto")], capture_output=True, timeout=30).returncode != 0
            assert (previous / "archive.junto").is_file()
            assert not (installed.parent / "Junto.previous.app").exists()
            assert not list(previous.rglob("*.app"))
            # This is the documented recovery command; the new application is never used.
            recovery = [str(previous / "recover"), "--test-configuration", str(public), str(fresh_public), "rollback", str(installed), str(pin)]
            backup = (previous / "archive.junto").read_bytes()
            (previous / "archive.junto").write_bytes(b"changed archive")
            result = subprocess.run(recovery, capture_output=True, timeout=30)
            assert result.returncode != 0 and version(installed) == "2.0.0"
            (previous / "archive.junto").write_bytes(backup)
            result = subprocess.run(recovery, capture_output=True, timeout=30)
            assert result.returncode == 0, result.stderr.decode()
            assert version(installed) == "1.0.0"
            assert requirement(installed) == expected
            assert subprocess.run([str(installed / "Contents/MacOS/junto")], capture_output=True, timeout=30).returncode == 0
            assert subprocess.run(recovery, capture_output=True, timeout=30).returncode != 0
            for identity_kind in ["adhoc", "other"]:
                bad = assemble("/usr/bin/true", root / identity_kind / "Junto.app", "3.0.0", tool)
                if identity_kind == "adhoc":
                    run("/usr/bin/codesign", "--force", "--sign", "-", str(bad))
                else:
                    other_identity = create(root / "other-identity")
                    other_pin = root / "other-certificate.sha1"
                    other_pin.write_text(other_identity)
                    other_environment = environment.copy()
                    other_environment.update(JUNTO_CODESIGN_IDENTITY=other_identity,
                        JUNTO_CODESIGN_KEYCHAIN=str(root / "other-identity/signing.keychain-db"), JUNTO_TEST_CERTIFICATE_FILE=str(other_pin))
                    sign(bad, other_environment)
                bad_release = root / (identity_kind + "-release")
                package(bad, bad_release, "3.0.0")
                for file in bad_release.iterdir():
                    shutil.copyfile(file, release / file.name)
                assert subprocess.run(update, capture_output=True, timeout=30).returncode != 0
                assert version(installed) == "1.0.0"
            print("18 update and archive-recovery assertions passed", flush=True)
        finally:
            if server:
                server.shutdown()
                server.server_close()
                thread.join()
            cleanup([keychain, root / "other-identity/signing.keychain-db"], before)
            assert run("security", "list-keychains", "-d", "user") == before
            assert run("security", "default-keychain", "-d", "user") == default_before
            print("Recovery search list before:\n" + before + "Recovery search list after:\n" + run("security", "list-keychains", "-d", "user"), flush=True)


if __name__ == "__main__":
    check(sys.argv[1])
