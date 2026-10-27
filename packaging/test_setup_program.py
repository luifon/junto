"""The real Windows uninstall dialogs, in an isolated installation and process job."""
import contextlib
import json
import os
from pathlib import Path
import shutil
import sys
import tempfile
import unittest
from unittest import mock

if os.name == "nt":
    import ctypes
    import window_frame
    from ctypes import wintypes
    window_frame.user32.GetDlgItemTextW.argtypes = [wintypes.HWND, ctypes.c_int, wintypes.LPWSTR, ctypes.c_int]
    window_frame.user32.GetDlgItemTextW.restype = wintypes.UINT
    window_frame.user32.GetDlgItem.argtypes = [wintypes.HWND, ctypes.c_int]
    window_frame.user32.GetDlgItem.restype = wintypes.HWND
    window_frame.user32.GetClassNameW.argtypes = [wintypes.HWND, wintypes.LPWSTR, ctypes.c_int]
    window_frame.user32.EnumChildWindows.argtypes = [wintypes.HWND, window_frame.ENUM, wintypes.LPARAM]
    sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools/windows"))
    from lock import Job


@unittest.skipUnless(os.name == "nt", "Windows message boxes")
class UninstallDialogs(unittest.TestCase):
    @contextlib.contextmanager
    def installation(self):
        target = Path(os.environ.get("CARGO_TARGET_DIR", Path(__file__).resolve().parents[1] / "target"))
        installer = target / "debug/junto-installer.exe"
        self.assertTrue(installer.is_file(), "build junto-installer before the dialog tests")
        with tempfile.TemporaryDirectory(prefix="junto-uninstall-dialog-") as temporary:
            root = Path(temporary)
            folder = root / "Local/Programs/Junto"
            folder.mkdir(parents=True)
            for name in ("junto.exe", "junto-recover.exe", "uninstall.exe"):
                shutil.copy2(installer, folder / name)
            (folder / "junto-package.json").write_text(json.dumps(dict(
                format=1, version="0.1.0", system="windows", package="directory", architecture="x86_64")))
            data = root / "Local/Junto"
            data.mkdir()
            (data / "kept.txt").write_text("keep")
            copies = root / "copies"
            copies.mkdir()
            environment = dict(LOCALAPPDATA=str(root / "Local"), APPDATA=str(root / "Roaming"),
                               TEMP=str(copies), TMP=str(copies))
            job = Job()
            with window_frame.kept_identity(), window_frame.kept_uninstall_entry(), \
                    mock.patch.dict(os.environ, environment):
                try:
                    yield job, folder, data, copies
                finally:
                    job.end()
                    job.kernel32.CloseHandle(job.handle)

    def dialog(self, job, text):
        def find():
            for number, handle in job.running():
                try:
                    for window in window_frame.windows_of(number):
                        buffer = ctypes.create_unicode_buffer(4096)
                        window_frame.user32.GetDlgItemTextW(window, 0xFFFF, buffer, len(buffer))
                        if text in buffer.value:
                            return window
                finally:
                    job.kernel32.CloseHandle(handle)
            return None
        return window_frame.until(text, find, 30)

    @staticmethod
    def answer(window, button):
        control = window_frame.user32.GetDlgItem(window, button)
        if not control:
            raise AssertionError(f"dialog button {button} is absent")
        window_frame.user32.SendMessageW(control, 0x00F5, 0, 0)  # BM_CLICK

    def dismiss(self, window):
        buttons = []
        def visit(control, _):
            kind = ctypes.create_unicode_buffer(256)
            window_frame.user32.GetClassNameW(control, kind, len(kind))
            if kind.value == "Button":
                buttons.append(control)
            return True
        window_frame.user32.EnumChildWindows(window, window_frame.ENUM(visit), 0)
        self.assertEqual(len(buttons), 1, "an information dialog has one button")
        window_frame.user32.SendMessageW(buttons[0], 0x00F5, 0, 0)

    def test_a_setup_without_payload_refuses_to_remove_its_containing_folder(self):
        with self.installation() as (job, folder, data, copies):
            downloads = copies / "downloads"
            downloads.mkdir()
            setup = downloads / "setup.exe"
            shutil.copy2(folder / "uninstall.exe", setup)
            sentinel = downloads / "personal.txt"
            sentinel.write_text("keep")
            process = job.start([str(setup), "--quiet"])
            self.assertNotEqual(process.wait(timeout=30), 0, "a setup without its payload accepted removal")
            self.assertEqual(sentinel.read_text(), "keep")
            process = job.start([str(setup), "--uninstall", "--quiet"])
            self.assertNotEqual(process.wait(timeout=30), 0, "an unrelated folder accepted removal")
            self.assertEqual(sentinel.read_text(), "keep")

    def test_the_data_question_defaults_to_no_and_enter_keeps_the_data(self):
        with self.installation() as (job, folder, data, copies):
            process = job.start([str(folder / "uninstall.exe"), "--uninstall"])
            self.answer(self.dialog(job, "Remove Junto from this account?"), 1)
            question = self.dialog(job, "Also remove your Junto data")
            default = window_frame.user32.SendMessageW(question, 0x0400, 0, 0) & 0xFFFF  # DM_GETDEFID
            self.answer(question, default)
            self.dismiss(self.dialog(job, "Junto was removed."))
            process.wait(timeout=30)
            self.assertEqual(default, 7, "Enter must choose No (IDNO)")
            self.assertEqual((data / "kept.txt").read_text(), "keep")
            self.assertFalse(folder.exists())


if __name__ == "__main__":
    unittest.main()
