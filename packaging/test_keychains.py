"""Keychain helpers: the lock, the restore of the search list, and the machine names."""
import os
import pathlib
import signal
import subprocess
import sys
import tempfile
import unittest
from types import SimpleNamespace
from unittest.mock import patch
import keychains


class KeychainTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.environment = patch.dict(os.environ, {"JUNTO_TEST_KEYCHAIN_LOCK": str(pathlib.Path(self.temporary.name) / "keychain.lock")})
        self.environment.start()
        self.addCleanup(self.environment.stop)

    def test_unrecognized_interactive_security_error_is_refused(self):
        result = SimpleNamespace(returncode=0, stdout="", stderr="permission denied\n")
        with patch.object(keychains.subprocess, "run", return_value=result):
            with self.assertRaises(RuntimeError):
                keychains.security_secret("unlock-keychain", "-p", "private password", "keychain")

    @unittest.skipIf(os.name == "nt", "POSIX signal restoration")
    def test_restore_ignores_interrupt_as_well_as_termination(self):
        def run(*args):
            if "-s" in args:
                self.assertIs(signal.getsignal(signal.SIGINT), signal.SIG_IGN)
                self.assertIs(signal.getsignal(signal.SIGTERM), signal.SIG_IGN)
                self.assertIs(signal.getsignal(signal.SIGHUP), signal.SIG_IGN)
            return '"original.keychain"\n'
        with patch.object(keychains, "run", run):
            with keychains.session():
                pass

    @unittest.skipIf(os.name == "nt", "POSIX keychain locking")
    def test_keychain_lock_is_independent_of_temp_directory(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            first, second = root / "first", root / "second"
            first.mkdir()
            second.mkdir()
            opened = []
            original = pathlib.Path.open
            def opened_path(path, *args, **kwargs):
                opened.append(path)
                return original(path, *args, **kwargs)
            with patch.object(pathlib.Path, "open", opened_path), patch.object(keychains, "run", return_value=""), patch.object(pathlib.Path, "home", return_value=root), patch.dict(os.environ):
                os.environ.pop("JUNTO_TEST_KEYCHAIN_LOCK", None)
                for directory in [first, second]:
                    with patch.object(tempfile, "gettempdir", return_value=str(directory)):
                        with keychains.session():
                            pass
            self.assertEqual(opened[0], opened[1])

    @unittest.skipIf(os.name == "nt", "POSIX keychain cleanup")
    def test_cleanup_restores_the_list_from_before_an_interrupted_signer(self):
        owned = pathlib.Path(self.temporary.name) / "deleted.keychain"
        owned.touch()
        calls = []
        def run(*args):
            calls.append(args)
            return '"original.keychain" "deleted.keychain"\n'
        with patch.object(keychains, "run", run):
            keychains.cleanup([owned], '"original.keychain"\n')
        self.assertEqual(calls[-1], ("security", "list-keychains", "-d", "user", "-s", "original.keychain"))

    @unittest.skipIf(os.name == "nt", "POSIX keychain locking")
    def test_a_second_process_cannot_take_the_search_list_lock(self):
        code = "import os,fcntl,sys; held=open(os.environ['JUNTO_TEST_KEYCHAIN_LOCK'],'a')\ntry:\n fcntl.flock(held,fcntl.LOCK_EX|fcntl.LOCK_NB)\nexcept BlockingIOError:\n sys.exit(0)\nsys.exit(1)"
        environment = os.environ.copy()
        environment["TMPDIR"] = self.temporary.name
        with patch.object(keychains, "run", return_value=""):
            with keychains.session():
                result = subprocess.run([sys.executable, "-c", code], env=environment, capture_output=True, timeout=10)
                self.assertEqual(result.returncode, 0, result.stderr)

    def test_supported_machine_spellings(self):
        from configuration import architecture
        for name in ["arm64", "aarch64"]:
            self.assertEqual(architecture(name), "aarch64")
        for name in ["x86_64", "AMD64"]:
            self.assertEqual(architecture(name), "x86_64")
        with self.assertRaisesRegex(ValueError, "unsupported release architecture"):
            architecture("unsupported")

