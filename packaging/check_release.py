#!/usr/bin/env python3
"""Build two product releases and update through the first build's compiled source.

The caller must hold tools/serial.sh for this entire command.
"""
import functools
import http.server
import os
import pathlib
from configuration import architecture as release_architecture
import shlex
import shutil
import subprocess
import tempfile
import threading

import check_dmg
from check_signing import requirement
from create_identity import create, run
from keychains import cleanup, signals
from programs import outcome, program


def check():
    repository = pathlib.Path(__file__).resolve().parent.parent
    tracked = [repository / "Cargo.toml", repository / "Cargo.lock", repository / "packaging/update-keys.json", repository / "packaging/freshness-keys.json", repository / "packaging/certificate.sha1"]
    originals = {path: path.read_bytes() for path in tracked}
    architecture = release_architecture()
    before = run("security", "list-keychains", "-d", "user")
    default_before = run("security", "default-keychain", "-d", "user")
    with signals(), tempfile.TemporaryDirectory(prefix="junto-release-") as temporary:
        root = pathlib.Path(temporary)
        secret, public = root / "update.seed", root / "public.json"
        run(str(repository / "target/debug/junto-release"), "keygen", str(secret), str(public))
        fresh_secret, fresh_public = root / "freshness.seed", root / "freshness.json"
        run(str(repository / "target/debug/junto-release"), "keygen", str(fresh_secret), str(fresh_public))
        keychain = root / "identity/signing.keychain-db"
        server = None
        thread = None
        try:
            identity = create(root / "identity")
            pin = root / "certificate.sha1"
            pin.write_text(identity)
            class CountedHandler(http.server.SimpleHTTPRequestHandler):
                def do_GET(self):
                    self.server.requests += 1
                    super().do_GET()
            handler = functools.partial(CountedHandler, directory=str(root / "broken-release"))
            server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
            server.requests = 0
            thread = threading.Thread(target=server.serve_forever)
            thread.start()
            environment = os.environ.copy()
            environment.update(JUNTO_UPDATE_SOURCE=f"http://127.0.0.1:{server.server_port}/",
                               JUNTO_UPDATE_KEY=str(secret), JUNTO_FRESHNESS_KEY=str(fresh_secret), JUNTO_CODESIGN_IDENTITY=identity,
                               JUNTO_CODESIGN_KEYCHAIN=str(keychain), JUNTO_TEST_UPDATE_KEYS_FILE=str(public),
                               JUNTO_TEST_CERTIFICATE_FILE=str(pin), JUNTO_TEST_FRESHNESS_KEYS_FILE=str(fresh_public))
            for number in [1, 2]:
                environment["JUNTO_TEST_BUILD_VERSION"] = f"{number}.0.0"
                version = environment["JUNTO_TEST_BUILD_VERSION"]
                subprocess.run(["sh", "packaging/release_locked.sh", str(root / f"v{number}")], cwd=repository, env=environment, check=True)
                binary = root / f"v{number}/Junto.app/Contents/MacOS/junto"
                assert run(str(binary), "--version").strip() == version + " (test configuration)"
                # The disk image beside the ZIP, signed with the same identity (ADR 0728).
                check_dmg.check(root / f"v{number}/Junto-{version}-macos-{architecture}.dmg",
                                requirement(root / f"v{number}/Junto.app"), identity)
                if number == 1:
                    shutil.copy2(repository / "target/release/junto-release", root / "updater-v1")
            other_identity = create(root / "other-identity")
            other_pin = root / "other-certificate.sha1"
            other_pin.write_text(other_identity)
            other = root / "other/Junto.app"
            shutil.copytree(root / "v2/Junto.app", other)
            other_env = environment.copy()
            other_env.update(JUNTO_CODESIGN_IDENTITY=other_identity,
                             JUNTO_CODESIGN_KEYCHAIN=str(root / "other-identity/signing.keychain-db"),
                             JUNTO_TEST_CERTIFICATE_FILE=str(other_pin))
            subprocess.run(["python3", str(repository / "packaging/sign.py"), str(other)], env=other_env, check=True)
            release_tool = str(repository / "target/release/junto-release")
            publish = ["publish", "macos", "bundle", architecture, str(other.parent), str(root / "other-release"), str(secret), str(fresh_secret)]
            result = subprocess.run([release_tool, *publish], capture_output=True, text=True)
            assert result.returncode != 0 and "test configuration requires" in result.stderr, outcome(result)
            result = subprocess.run([release_tool, "--test-configuration", *publish], capture_output=True, text=True)
            assert result.returncode != 0 and "different project signing certificate" in result.stderr, outcome(result)
            broken = root / "broken/Junto.app"
            shutil.copytree(root / "v2/Junto.app", broken)
            program(broken / "Contents/MacOS/junto", 1)
            subprocess.run(["python3", str(repository / "packaging/sign.py"), str(broken)], env=environment, check=True)
            run(str(root / "updater-v1"), "package", str(broken.parent), str(root / "broken-release"), str(secret), str(fresh_secret), "2.0.0", "macos", "bundle", architecture)
            # The source keeps the release of every version: the first update downloads the
            # installed version for its record (ADR 0726).
            for name in [f"release-1.0.0-macos-bundle-{architecture}.json", f"junto-1.0.0-macos-bundle-{architecture}.junto"]:
                shutil.copyfile(root / "v1" / name, root / "broken-release" / name)
            run("/usr/bin/ditto", "-x", "-k", str(root / f"v1/Junto-1.0.0-macos-{architecture}.zip"), str(root / "installed"))
            installed = root / "installed/Junto.app"
            expected = requirement(installed)
            assert requirement(root / "v2/Junto.app") == expected
            installed.parent.chmod(0o555)
            try:
                result = subprocess.run([str(root / "updater-v1"), "update-compiled", str(installed)], capture_output=True, text=True, timeout=30)
            finally:
                installed.parent.chmod(0o755)
            assert result.returncode != 0 and "move Junto" in result.stderr, outcome(result)
            assert server.requests == 0, "a read-only location contacted the source"
            run(str(root / "updater-v1"), "update-compiled", str(installed))
            result = subprocess.run([str(installed / "Contents/MacOS/junto"), "--version"], capture_output=True, text=True, timeout=30)
            assert result.returncode == 1, outcome(result)
            assert requirement(installed) == expected
            previous = installed.parent / ".Junto.app.junto-update/record/previous"
            assert (previous / "archive.junto").is_file()
            assert not list((installed.parent / ".Junto.app.junto-update").rglob("*.app"))
            run(str(previous / "recover"), "rollback", str(installed))
            assert run(str(installed / "Contents/MacOS/junto"), "--version").strip() == "1.0.0 (test configuration)"
            assert requirement(installed) == expected
            print("Real release: 13 assertions passed, including archive recovery while the new executable fails", flush=True)
            bad_keys = root / "invalid-public.json"
            bad_keys.write_text("invalid JSON")
            for overrides, diagnostic in [({"JUNTO_UPDATE_SOURCE": "http://example.invalid/"}, "invalid update source"),
                                          ({"JUNTO_TEST_UPDATE_KEYS_FILE": str(bad_keys)}, "invalid update keys")]:
                invalid = os.environ.copy()
                invalid.update(overrides)
                result = subprocess.run(["cargo", "check", "--locked", "-p", "junto-client-core", "--lib"],
                                        cwd=repository, env=invalid, capture_output=True, text=True)
                assert result.returncode != 0 and diagnostic in result.stderr, outcome(result)
            subprocess.run(["cargo", "check", "--locked", "-p", "junto-client-core", "--lib"],
                           cwd=repository, check=True)
            print("Build configuration: 2 intended refusals and 1 valid build passed", flush=True)

        finally:
            if server:
                server.shutdown()
                server.server_close()
                thread.join()
            cleanup([keychain, root / "other-identity/signing.keychain-db"], before)
            assert all(path.read_bytes() == content for path, content in originals.items()), "tracked configuration changed"
            after = run("security", "list-keychains", "-d", "user")
            assert before == after
            assert default_before == run("security", "default-keychain", "-d", "user")
            print("Release search list before:\n" + before + "Release search list after:\n" + after, flush=True)


if __name__ == "__main__":
    check()
