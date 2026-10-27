#!/usr/bin/env python3
"""Sign with a dedicated keychain and restore the search list within this command."""
import os
import pathlib
import sys


from keychains import run, session


def sign(bundle):
    """Signs `Junto.app` and its recovery program, or a disk image of the release (ADR 0728),
    with the project certificate, and requires the certificate in the designated requirement."""
    identity = os.environ.get("JUNTO_CODESIGN_IDENTITY", "")
    if identity in ("", "-"):
        raise ValueError("a code-signing certificate is required")
    pin = pathlib.Path(os.environ.get("JUNTO_TEST_CERTIFICATE_FILE", pathlib.Path(__file__).with_name("certificate.sha1"))).read_text().strip()
    if len(pin) != 40 or any(c not in "0123456789abcdefABCDEF" for c in pin) or identity.upper() != pin.upper():
        raise ValueError("the identity must equal the configured project certificate")
    keychain = str(pathlib.Path(os.environ["JUNTO_CODESIGN_KEYCHAIN"]).resolve(strict=True))
    with session() as before:
        run("security", "list-keychains", "-d", "user", "-s", *dict.fromkeys([*before, keychain]))
        recovery = pathlib.Path(bundle) / "Contents/MacOS/junto-recover"
        if recovery.is_file():
            run("/usr/bin/codesign", "--force", "--timestamp=none", "--keychain", keychain, "--sign", identity, str(recovery))
        run("/usr/bin/codesign", "--force", "--timestamp=none", "--keychain", keychain, "--sign", identity, bundle)
        if str(bundle).endswith(".dmg"):
            run("/usr/bin/codesign", "--verify", "--strict", bundle)
        else:
            run("/usr/bin/codesign", "--verify", "--deep", "--strict", bundle)
        required = run("/usr/bin/codesign", "-d", "-r-", bundle).lower()
        if f'certificate leaf = h"{pin.lower()}"' not in required:
            raise ValueError("the signature differs from the project certificate")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit("usage: packaging/sign.py <Junto.app | image.dmg>")
    try:
        sign(sys.argv[1])
    except (RuntimeError, ValueError, KeyError) as error:
        sys.exit(str(error))
