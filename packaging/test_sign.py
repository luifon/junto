"""Signing refuses identities that cannot establish a stable application identity."""
import os
import pathlib
import subprocess
import sys
import unittest


class SigningTests(unittest.TestCase):
    def test_adhoc_identity_is_refused_without_touching_a_keychain(self):
        environment = os.environ.copy()
        environment["JUNTO_CODESIGN_IDENTITY"] = "-"
        result = subprocess.run([sys.executable, str(pathlib.Path(__file__).with_name("sign.py")), "missing.app"],
                                env=environment, capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("certificate", result.stderr)


class IdentityTests(unittest.TestCase):
    def test_certificate_creation_refuses_a_repository_directory(self):
        import importlib.util
        spec = importlib.util.spec_from_file_location("identity", pathlib.Path(__file__).with_name("create_identity.py"))
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        with self.assertRaises(ValueError):
            module.create(pathlib.Path(__file__).resolve().parent / "forbidden-key")


class CertificateTests(unittest.TestCase):
    @unittest.skipUnless(sys.platform == "darwin", "real macOS code signatures")
    def test_signing_works_when_the_default_openssl_configuration_adds_extensions(self):
        # The default configuration of OpenSSL 1.1 adds the extensions of a certificate
        # authority to every self-signed certificate.
        import tempfile
        from unittest.mock import patch
        import check_signing
        with tempfile.TemporaryDirectory(prefix="junto-openssl-") as temporary:
            configuration = pathlib.Path(temporary) / "authority.cnf"
            configuration.write_text("[ req ]\ndistinguished_name = name\nx509_extensions = authority\n"
                                     "[ name ]\n[ authority ]\nbasicConstraints = critical,CA:true\n")
            with patch.dict(os.environ, {"OPENSSL_CONF": str(configuration)}):
                check_signing.check()


class EmptyIdentityTests(unittest.TestCase):
    def test_empty_identity(self):
        import sign
        from unittest.mock import patch
        with patch.dict(os.environ, {"JUNTO_CODESIGN_IDENTITY": ""}):
            with self.assertRaisesRegex(ValueError, "certificate"):
                sign.sign("missing.app")


if __name__ == "__main__":
    unittest.main()
