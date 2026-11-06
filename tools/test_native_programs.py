"""Unit tests of tools/native_downloads.py, tools/with_native_livekit.py and
tools/with_native_ffmpeg.py. They download nothing and start no server: the Windows job of CI
starts the real ones."""

import hashlib
import io
import os
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
        text = with_native_livekit.config(chosen, "room:\n  departure_timeout: 60\n", "linux")
        self.assertIn("port: 18500\n", text)
        self.assertIn("  udp_port: 18502\n", text)
        self.assertIn("  tcp_port: 0\n", text)
        self.assertIn("    includes: [127.0.0.1/32]\n", text)
        self.assertIn("  node_ip: 127.0.0.1\n", text)
        self.assertIn("  enable_loopback_candidate: true\n", text)
        self.assertIn("    - http://127.0.0.1:18510/internal/media/notifications\n", text)
        self.assertTrue(text.endswith("room:\n  departure_timeout: 60\n"))
        environment = with_native_livekit.command_environment({"OTHER": "kept"}, chosen)
        self.assertEqual(environment["JUNTO_TEST_LIVEKIT_URL"], "ws://127.0.0.1:18500")
        self.assertEqual(environment["JUNTO_TEST_LIVEKIT_API_URL"], "http://127.0.0.1:18500")
        self.assertEqual(environment["JUNTO_TEST_WEBHOOK_ADDRESS"], "127.0.0.1:18510")
        self.assertEqual(environment["OTHER"], "kept")
        self.assertNotIn("JUNTO_TEST_LIVEKIT_CONTAINER", environment)

    def test_on_windows_the_udp_port_is_also_on_the_private_addresses_of_the_machine(self):
        # Windows routes no UDP between an address of the network and 127.0.0.1, and the call
        # library offers no candidate on the loopback interface.
        chosen = with_native_livekit.settings({})
        text = with_native_livekit.config(chosen, "", "win32")
        self.assertIn("    includes: [127.0.0.1/32, 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16]\n", text)
        self.assertIn("  tcp_port: 0\n", text)
        self.assertIn("  use_external_ip: false\n", text)
        self.assertNotIn("node_ip", text)

    def test_the_job_only_checks_the_rule_of_the_firewall_that_the_setup_created(self):
        program = r"C:\Users\ci\.cache\junto-livekit\livekit-1.13.9\livekit-server.exe"
        self.assertEqual(with_native_livekit.rule_command(),
                         ["netsh", "advfirewall", "firewall", "show", "rule", "name=junto-test-livekit", "verbose"])

        def shown(program_line, action="Block", enabled="Yes", profiles="Domain,Private,Public"):
            return ("\r\nRule Name:                            junto-test-livekit\r\n"
                    "----------------------------------------------------------------------\r\n"
                    f"Enabled:                              {enabled}\r\nDirection:                            In\r\n"
                    f"Profiles:                             {profiles}\r\nLocalIP:                              Any\r\n"
                    f"Program:                              {program_line}\r\n"
                    f"Action:                               {action}\r\n\r\nOk.\r\n")

        self.assertTrue(with_native_livekit.blocks(shown(program), program))
        self.assertTrue(with_native_livekit.blocks(shown(program.upper()), program))
        other = program.replace("1.13.9", "1.13.8")
        self.assertTrue(with_native_livekit.blocks(shown(other) + shown(program), program))
        for wrong in [shown(other), shown(program, action="Allow"), shown(program, enabled="No"),
                      shown(program, profiles="Private"), "No rules match the specified criteria.\r\n"]:
            self.assertFalse(with_native_livekit.blocks(wrong, program), wrong)

        class Done:
            def __init__(self, code, stdout):
                self.returncode, self.stdout = code, stdout

        commands = []

        def run(command, **_):
            commands.append(command)
            return Done(0, shown(program))

        self.assertTrue(with_native_livekit.has_rule(program, run))
        self.assertEqual(commands, [with_native_livekit.rule_command()])
        self.assertFalse(with_native_livekit.has_rule(program, lambda command, **_: Done(1, "No rules match")))
        self.assertIn("tools/ci/ci.sh setup-windows", with_native_livekit.no_rule(program))

    def test_the_setup_blocks_the_program_that_the_job_checks(self):
        setup = (REPOSITORY / "tools" / "windows" / "ci-setup.ps1").read_text()
        self.assertIn(f"$LiveKitVersion = '{with_native_livekit.VERSION}'", setup)
        self.assertIn(f"$LiveKitRule = '{with_native_livekit.RULE}'", setup)
        self.assertIn(r"('.cache\junto-livekit\livekit-' + $LiveKitVersion + '\livekit-server.exe')", setup)
        for setting in ["dir=in", "action=block", "profile=any", "enable=yes"]:
            self.assertIn(setting, setup)

    def test_the_program_is_extracted_once_beside_its_download(self):
        directory = pathlib.Path(tempfile.mkdtemp(prefix="junto-livekit-"))
        self.addCleanup(shutil.rmtree, directory, True)
        archive = directory / "livekit.zip"
        archive.write_bytes(zip_bytes({"livekit-server.exe": b"program"}))
        extractions = []

        def extract(path, members, destination):
            extractions.append(path)
            return native_downloads.extract(path, members, destination)

        for _ in range(2):
            program = with_native_livekit.installed(archive, "livekit-server.exe", directory, "a" * 64, extract)
            self.assertEqual(program, directory / f"livekit-{with_native_livekit.VERSION}" / "livekit-server.exe")
            self.assertEqual(program.read_bytes(), b"program")
        self.assertEqual(len(extractions), 1)
        with_native_livekit.installed(archive, "livekit-server.exe", directory, "b" * 64, extract)
        self.assertEqual(len(extractions), 2)

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

    def test_the_command_finds_the_programs_on_its_path_and_gets_no_names_of_them(self):
        # A server of the tests refuses JUNTO_FFMPEG without JUNTO_RECORDINGS_DIR.
        base = {"PATH": "before", "JUNTO_FFMPEG": "a", "JUNTO_FFPROBE": "b", "OTHER": "c"}
        given = with_native_ffmpeg.environment(pathlib.Path("programs"), base)
        self.assertEqual(given, {"PATH": f"programs{os.pathsep}before", "OTHER": "c"})


if __name__ == "__main__":
    unittest.main()
