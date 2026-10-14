#!/usr/bin/env python3
"""Exercise signing with small test bundles; no application is launched."""
import os
import pathlib
import shlex
import subprocess
import tempfile

from bundle import assemble
from create_identity import create, run
from keychains import cleanup, signals
from programs import outcome, program


def requirement(bundle):
    result = subprocess.run(["/usr/bin/codesign", "-d", "-r-", str(bundle)], capture_output=True, text=True, timeout=30)
    if result.returncode:
        raise RuntimeError(outcome(result))
    return next(line for line in (result.stdout + result.stderr).splitlines() if "designated =>" in line)


def check():
    before = run("security", "list-keychains", "-d", "user")
    default_before = run("security", "default-keychain", "-d", "user")
    with signals(), tempfile.TemporaryDirectory(prefix="junto signing with spaces-") as temporary:
        root = pathlib.Path(temporary)
        keychain = root / "identity/signing.keychain-db"
        try:
            fingerprint = create(root / "identity")
            environment = os.environ.copy()
            environment["JUNTO_CODESIGN_IDENTITY"] = fingerprint
            environment["JUNTO_CODESIGN_KEYCHAIN"] = str(keychain)
            pin = root / "certificate.sha1"
            pin.write_text(environment["JUNTO_CODESIGN_IDENTITY"])
            environment["JUNTO_TEST_CERTIFICATE_FILE"] = str(pin)
            succeeding = program(root / "exit-0", 0)
            bundles = [assemble(succeeding, root / f"v{n}/Junto.app", f"{n}.0.0") for n in [1, 2]]
            for bundle in bundles:
                subprocess.run([str(pathlib.Path(__file__).with_name("sign.sh")), str(bundle)], env=environment, check=True, timeout=30)
            assert requirement(bundles[0]) == requirement(bundles[1]), "certificate identity changed"
            adhoc = assemble(succeeding, root / "adhoc/Junto.app", "3.0.0")
            run("/usr/bin/codesign", "--force", "--sign", "-", str(adhoc))
            assert requirement(adhoc) != requirement(bundles[0]), "ad-hoc identity was accepted"
            print("2 signature comparisons passed")
        finally:
            cleanup([keychain], before)
            after = run("security", "list-keychains", "-d", "user")
            assert before == after, "keychain search list changed"
            print("Search list before:\n" + before + "Search list after:\n" + after, flush=True)
    after = run("security", "list-keychains", "-d", "user")
    assert before == after, "keychain search list changed"
    assert default_before == run("security", "default-keychain", "-d", "user"), "default keychain changed"


if __name__ == "__main__":
    check()
