"""Release configuration and cleanup contracts, without system keychain changes."""
import os
import pathlib
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
import sign


class Corrections(unittest.TestCase):
    def test_signing_requires_the_pinned_certificate(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            pin = root / "certificate.sha1"
            pin.write_text("A" * 40)
            with patch.dict(os.environ, {"JUNTO_CODESIGN_IDENTITY": "B" * 40,
                                         "JUNTO_TEST_CERTIFICATE_FILE": str(pin)}):
                with self.assertRaisesRegex(ValueError, "certificate"):
                    sign.sign("missing.app")
            pin.write_text("")
            with patch.dict(os.environ, {"JUNTO_CODESIGN_IDENTITY": "A" * 40,
                                         "JUNTO_TEST_CERTIFICATE_FILE": str(pin)}):
                with self.assertRaisesRegex(ValueError, "certificate"):
                    sign.sign("missing.app")

    @unittest.skipIf(os.name == "nt", "macOS product release script")
    def test_release_check_never_writes_tracked_configuration(self):
        import check_release
        repository = pathlib.Path(__file__).resolve().parent.parent
        tracked = [repository / "Cargo.toml", repository / "Cargo.lock", repository / "packaging/update-keys.json", repository / "packaging/freshness-keys.json", repository / "packaging/certificate.sha1"]
        originals = {path: path.read_bytes() for path in tracked}
        write_text, write_bytes = pathlib.Path.write_text, pathlib.Path.write_bytes
        def guarded(writer):
            def write(path, *args, **kwargs):
                if path in originals:
                    raise PermissionError("test attempted to rewrite tracked configuration")
                return writer(path, *args, **kwargs)
            return write
        def run(*args):
            if args[0] == "security":
                return '"original.keychain"\n'
            if args[1] == "keygen":
                pathlib.Path(args[2]).write_bytes(os.urandom(32))
                pathlib.Path(args[3]).write_text("[]")
            return ""
        def build(*args, **kwargs):
            environment = kwargs["env"]
            self.assertEqual(environment["JUNTO_TEST_BUILD_VERSION"], "1.0.0")
            self.assertTrue(pathlib.Path(environment["JUNTO_TEST_UPDATE_KEYS_FILE"]).is_file())
            self.assertTrue(pathlib.Path(environment["JUNTO_TEST_FRESHNESS_KEYS_FILE"]).is_file())
            self.assertTrue(pathlib.Path(environment["JUNTO_TEST_CERTIFICATE_FILE"]).is_file())
            self.assertTrue(all(path.read_bytes() == content for path, content in originals.items()))
            raise RuntimeError("build interrupted")
        with patch.object(pathlib.Path, "write_text", guarded(write_text)), \
             patch.object(pathlib.Path, "write_bytes", guarded(write_bytes)), \
             patch.object(check_release, "run", run), \
             patch.object(check_release, "create", return_value="A" * 40), \
             patch.object(check_release, "cleanup"), \
             patch.object(check_release.subprocess, "run", build):
            with self.assertRaisesRegex(RuntimeError, "build interrupted"):
                check_release.check()
        self.assertTrue(all(path.read_bytes() == content for path, content in originals.items()))

    def test_standalone_signing_tests_include_identity_and_empty_identity(self):
        result = subprocess.run([sys.executable, str(pathlib.Path(__file__).with_name("test_sign.py")), "-v"],
                                capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("test_certificate_creation_refuses_a_repository_directory", result.stderr)
        self.assertIn("test_empty_identity", result.stderr)

    @unittest.skipIf(os.name == "nt", "macOS keychain lifecycle")
    def test_cleanup_restores_search_list_even_when_delete_fails(self):
        import keychains
        with tempfile.TemporaryDirectory() as temporary:
            owned = pathlib.Path(temporary) / "keychain"
            owned.touch()
            calls = []
            def run(*args):
                calls.append(args)
                if args[1] == "delete-keychain":
                    raise RuntimeError("injected delete failure")
                return '"original.keychain"\n'
            with patch.object(keychains, "run", run):
                with self.assertRaisesRegex(RuntimeError, "injected"):
                    keychains.cleanup([owned])
            self.assertEqual(calls[-1], ("security", "list-keychains", "-d", "user", "-s", "original.keychain"))

    def test_password_is_only_in_standard_input(self):
        import keychains
        from types import SimpleNamespace
        with patch.object(keychains.subprocess, "run", return_value=SimpleNamespace(returncode=0, stderr="")) as call:
            keychains.security_secret("create-keychain", "-p", "test password", "test.keychain")
        self.assertEqual(call.call_args.args[0], ["security", "-i"])
        self.assertIn("test password", call.call_args.kwargs["input"])

    @unittest.skipIf(os.name == "nt", "POSIX release entry point")
    def test_failed_signing_removes_partial_release_output(self):
        import shlex
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            cargo = root / "cargo"
            cargo.write_text("#!/bin/sh\nexit 0\n")
            cargo.chmod(0o755)
            python = root / "python3"
            python.write_text("#!/bin/sh\ncase \"$1\" in\n *bundle.py) mkdir -p \"$3\" ;;\n *sign.py) exit 23 ;;\n *) exec " + shlex.quote(sys.executable) + " \"$@\" ;;\nesac\n")
            python.chmod(0o755)
            environment = os.environ.copy()
            environment.update(PATH=str(root) + os.pathsep + environment["PATH"],
                               JUNTO_CODESIGN_IDENTITY="A" * 40,
                               JUNTO_CODESIGN_KEYCHAIN=str(root / "unused.keychain"))
            output = root / "release"
            result = subprocess.run(["sh", "packaging/release_locked.sh", str(output)],
                                    env=environment, capture_output=True, text=True,
                                    cwd=pathlib.Path(__file__).resolve().parent.parent)
            self.assertEqual(result.returncode, 23, result.stderr)
            self.assertFalse(output.exists())
