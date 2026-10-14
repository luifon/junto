#!/usr/bin/env python3
"""Create a permanent self-signed code-signing identity outside every repository."""
import os
import pathlib
import secrets
import shutil
import sys


from keychains import run, session, security_secret

# The OpenSSL of the system with the configuration of this directory: another OpenSSL in PATH
# can have another default configuration, which adds extensions to the certificate.
OPENSSL = "/usr/bin/openssl"
CONFIGURATION = pathlib.Path(__file__).with_name("code-signing.cnf")


def create(directory):
    directory = pathlib.Path(directory).resolve()
    if any((parent / ".git").exists() for parent in (directory, *directory.parents)):
        raise ValueError("the signing identity must be outside a repository")
    previous_mask = os.umask(0o077)
    try:
        with session():
            return create_locked(directory)
    finally:
        os.umask(previous_mask)


def create_locked(directory):
    created = False
    keychain_created = False
    keychain = directory / "signing.keychain-db"
    try:
        directory.mkdir(mode=0o700, exist_ok=False)
        created = True
        password = secrets.token_hex(32)
        (directory / "keychain-password").write_text(password)
        run(OPENSSL, "req", "-x509", "-config", str(CONFIGURATION), "-newkey", "rsa:2048", "-sha256",
            "-days", "3650", "-nodes", "-keyout", str(directory / "private-key.pem"),
            "-out", str(directory / "certificate.pem"))
        security_secret("create-keychain", "-p", password, str(keychain))
        keychain_created = True
        run("security", "set-keychain-settings", str(keychain))
        security_secret("unlock-keychain", "-p", password, str(keychain))
        run("security", "import", str(directory / "private-key.pem"), "-k", str(keychain), "-T", "/usr/bin/codesign")
        run("security", "import", str(directory / "certificate.pem"), "-k", str(keychain))
        security_secret("set-key-partition-list", "-S", "apple-tool:,apple:,codesign:", "-s", "-k", password, str(keychain))
        fingerprint = run(OPENSSL, "x509", "-in", str(directory / "certificate.pem"), "-noout", "-fingerprint", "-sha1")
        return fingerprint.strip().split("=", 1)[1].replace(":", "")
    except BaseException:
        try:
            if keychain_created:
                run("security", "delete-keychain", str(keychain))
        finally:
            if created:
                shutil.rmtree(directory)
        raise


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit("usage: packaging/create_identity.py <new private directory outside a repository>")
    print(create(sys.argv[1]))
