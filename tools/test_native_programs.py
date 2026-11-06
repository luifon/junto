"""Unit tests of tools/native_downloads.py, tools/with_native_livekit.py and
tools/with_native_ffmpeg.py. They download nothing and start no server: the Windows job of CI
starts the real ones."""

import hashlib
import io
import pathlib
import re
import shutil
import sys
import tarfile
import tempfile
import unittest
import zipfile

TOOLS = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(TOOLS))

import native_downloads  # noqa: E402
import with_native_ffmpeg  # noqa: E402
import with_native_livekit  # noqa: E402

REPOSITORY = TOOLS.parent


def zip_bytes(members):
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w") as archive:
        for name, data in members.items():
            archive.writestr(name, data)
    return buffer.getvalue()


def tar_bytes(members):
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w:gz") as archive:
        for name, data in members.items():
            info = tarfile.TarInfo(name)
            info.size = len(data)
            archive.addfile(info, io.BytesIO(data))
    return buffer.getvalue()


class Downloads(unittest.TestCase):
    def setUp(self):
        self.directory = pathlib.Path(tempfile.mkdtemp(prefix="junto-downloads-"))
        self.addCleanup(shutil.rmtree, self.directory, True)

    def test_a_download_is_kept_only_with_the_pinned_sha256(self):
        data = b"the archive"
        expected = hashlib.sha256(data).hexdigest()
        gets = []

        def get(url, path):
            gets.append(url)
            path.write_bytes(data)

        path = native_downloads.fetch("Program", "https://example.test/a/program.zip", expected, self.directory, get)
        self.assertEqual(path, self.directory / "program.zip")
        self.assertEqual(path.read_bytes(), data)
        native_downloads.fetch("Program", "https://example.test/a/program.zip", expected, self.directory, get)
        self.assertEqual(len(gets), 1, "a kept download with the pinned SHA-256 is not downloaded again")
        with self.assertRaisesRegex(native_downloads.Failure, "not the pinned"):
            native_downloads.fetch("Program", "https://example.test/a/other.zip", "0" * 64, self.directory, get)
        self.assertFalse((self.directory / "other.zip").exists())
        self.assertFalse((self.directory / "other.zip.part").exists())

    def test_single_files_come_out_of_a_zip_and_a_tar_under_their_last_name(self):
        for name, data in [("archive.zip", zip_bytes({"top/bin/one.exe": b"1", "top/doc/x": b"x"})),
                           ("archive.tar.gz", tar_bytes({"top/bin/one.exe": b"1", "top/doc/x": b"x"}))]:
            with self.subTest(archive=name):
                archive = self.directory / name
                archive.write_bytes(data)
                out = self.directory / f"out-{name}"
                [one] = native_downloads.extract(archive, ["top/bin/one.exe"], out)
                self.assertEqual(one, out / "one.exe")
                self.assertEqual(one.read_bytes(), b"1")
                self.assertEqual(sorted(path.name for path in out.iterdir()), ["one.exe"])
                with self.assertRaisesRegex(native_downloads.Failure, "holds no top/bin/two.exe"):
                    native_downloads.extract(archive, ["top/bin/two.exe"], out)


class LiveKit(unittest.TestCase):
    def test_the_version_is_that_of_the_image_of_the_tests(self):
        pins = (REPOSITORY / "tools" / "ci" / "pins.sh").read_text()
        image = re.search(r"livekit/livekit-server:v([0-9.]+)@", pins).group(1)
        self.assertEqual(with_native_livekit.VERSION, image)
        for name, _, sha256 in with_native_livekit.RELEASES.values():
            self.assertIn(with_native_livekit.VERSION, name)
            self.assertRegex(sha256, "^[0-9a-f]{64}$")

    def test_no_release_for_another_system(self):
        with self.assertRaisesRegex(native_downloads.Failure, "darwin"):
            with_native_livekit.release("darwin")

    def test_the_server_has_the_ports_of_the_docker_script_and_binds_only_the_loopback_address(self):
        chosen = with_native_livekit.settings({"JUNTO_TEST_LIVEKIT_PORT": "18500", "JUNTO_TEST_LIVEKIT_UDP_PORT": "18502",
                                               "JUNTO_TEST_WEBHOOK_ADDRESS": "127.0.0.1:18510"})
        text = with_native_livekit.config(chosen, "room:\n  departure_timeout: 60\n")
        self.assertIn("port: 18500\n", text)
        self.assertIn("  udp_port: 18502\n", text)
        self.assertIn("  tcp_port: 0\n", text)
        self.assertIn("    includes: [127.0.0.1/32]\n", text)
        self.assertIn("  enable_loopback_candidate: true\n", text)
        self.assertIn("    - http://127.0.0.1:18510/internal/media/notifications\n", text)
        self.assertTrue(text.endswith("room:\n  departure_timeout: 60\n"))
        environment = with_native_livekit.command_environment({"OTHER": "kept"}, chosen)
        self.assertEqual(environment["JUNTO_TEST_LIVEKIT_URL"], "ws://127.0.0.1:18500")
        self.assertEqual(environment["JUNTO_TEST_LIVEKIT_API_URL"], "http://127.0.0.1:18500")
        self.assertEqual(environment["JUNTO_TEST_WEBHOOK_ADDRESS"], "127.0.0.1:18510")
        self.assertEqual(environment["OTHER"], "kept")
        self.assertNotIn("JUNTO_TEST_LIVEKIT_CONTAINER", environment)

    def test_a_webhook_address_on_another_host_is_refused(self):
        self.assertEqual(with_native_livekit.webhook_port("127.0.0.1:18510"), "18510")
        for address in ("192.0.2.4:18510", "localhost:18510", "127.0.0.1:"):
            with self.subTest(address=address):
                with self.assertRaisesRegex(native_downloads.Failure, "127.0.0.1:<port>"):
                    with_native_livekit.webhook_port(address)

    def test_the_defaults_are_those_of_the_docker_script(self):
        script = (REPOSITORY / "tools" / "with-test-livekit.sh").read_text()
        chosen = with_native_livekit.settings({})
        for variable, value in [("JUNTO_TEST_LIVEKIT_PORT", chosen["port"]),
                                ("JUNTO_TEST_LIVEKIT_UDP_PORT", chosen["udp_port"]),
                                ("JUNTO_TEST_WEBHOOK_ADDRESS", chosen["webhook"]),
                                ("JUNTO_TEST_LIVEKIT_API_KEY", chosen["key"]),
                                ("JUNTO_TEST_LIVEKIT_API_SECRET", chosen["secret"])]:
            self.assertIn(f"${{{variable}:-{value}}}", script)


class FFmpeg(unittest.TestCase):
    def test_the_version_is_that_of_the_server_image(self):
        dockerfile = (REPOSITORY / "deploy" / "images" / "server" / "Dockerfile").read_text()
        self.assertIn(f"ARG FFMPEG_VERSION={with_native_ffmpeg.VERSION}\n", dockerfile)
        self.assertRegex(with_native_ffmpeg.SHA256, "^[0-9a-f]{64}$")

    def test_the_programs_are_extracted_once_for_a_download(self):
        directory = pathlib.Path(tempfile.mkdtemp(prefix="junto-ffmpeg-"))
        self.addCleanup(shutil.rmtree, directory, True)
        archive = directory / "build.zip"
        archive.write_bytes(zip_bytes({name: name.encode() for name in with_native_ffmpeg.PROGRAMS}))
        extractions = []

        def extract(path, members, destination):
            extractions.append(path)
            return native_downloads.extract(path, members, destination)

        for _ in range(2):
            ffmpeg, ffprobe = with_native_ffmpeg.programs(directory, lambda *_: archive, extract)
            self.assertEqual((ffmpeg.name, ffprobe.name), ("ffmpeg.exe", "ffprobe.exe"))
            self.assertEqual(ffprobe.read_bytes(), with_native_ffmpeg.PROGRAMS[1].encode())
        self.assertEqual(len(extractions), 1)


if __name__ == "__main__":
    unittest.main()
