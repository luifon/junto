"""Bundle construction tests; no application is started and no identity is imported."""
import importlib.util
import pathlib
import plistlib
import tempfile
import unittest


class BundleTests(unittest.TestCase):
    def test_bundle_uses_the_requested_workspace_version_and_permission_texts(self):
        spec = importlib.util.spec_from_file_location("bundle", pathlib.Path(__file__).with_name("bundle.py"))
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        with tempfile.TemporaryDirectory(prefix="junto-bundle-") as temporary:
            root = pathlib.Path(temporary)
            binary = root / "junto"
            binary.write_bytes(b"not executable in this test")
            bundle = module.assemble(binary, root / "Junto.app", "0.2.3")
            info = plistlib.loads((bundle / "Contents/Info.plist").read_bytes())
            self.assertEqual(info["CFBundleIdentifier"], "org.junto.app")
            self.assertEqual(info["CFBundleVersion"], "0.2.3")
            self.assertEqual(info["CFBundleShortVersionString"], "0.2.3")
            self.assertEqual(info["CFBundleExecutable"], "junto")
            for item in ["NSMicrophoneUsageDescription", "NSCameraUsageDescription", "NSScreenCaptureUsageDescription"]:
                self.assertTrue(info[item])
            self.assertTrue((bundle / "Contents/Resources/Junto.icns").is_file())
            self.assertEqual((bundle / "Contents/MacOS/junto").read_bytes(), binary.read_bytes())
            with self.assertRaises(FileExistsError):
                module.assemble(binary, bundle, "0.2.4")


if __name__ == "__main__":
    unittest.main()
