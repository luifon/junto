"""The drivers of the deployment tests read the owner code from the standard output of the
server container, also when Docker placed a log record between the lines of its notice.

A fake `docker` program stands for Docker: `docker compose logs` writes both streams of the
container into its standard output in the order that Docker read them, as Docker Compose
does, and `docker logs` writes each stream of the container to its own stream, as Docker does.
"""

import argparse
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))

import content_driver  # noqa: E402
import recording_driver  # noqa: E402

NOTICE = (
    "This server has no owner. The first client that presents this one-time code becomes the "
    "server owner. A restart replaces the code.\n\n    {code}\n\n"
)
RECORD = '{"level":"INFO","fields":{"message":"listening","address":"0.0.0.0:8080"}}\n'
FIRST = "AAAA-BBBB-CCCC-DDDD-EEEE"
SECOND = "FFFF-GGGG-HHHH-JJJJ-KKKK"


def fake_docker(directory: Path, starts: list[str]) -> None:
    """Writes a `docker` program into `directory` for a server container that started once
    for each code of `starts`. In each start the server wrote its notice to standard output
    in two parts, and Docker read the record of standard error that followed it before the
    second part."""
    parts = [NOTICE.format(code=code).split("\n\n    ", 1) for code in starts]
    output = "".join(f"{first}\n\n    {second}" for first, second in parts)
    merged = "".join(f"{first}\n\n{RECORD}    {second}" for first, second in parts)
    program = directory / "docker"
    program.write_text(
        "#!/usr/bin/env python3\n"
        "import sys\n"
        "arguments = sys.argv[1:]\n"
        f"output, merged, record = {output!r}, {merged!r}, {RECORD!r}\n"
        "if arguments[0] == 'compose' and 'logs' in arguments:\n"
        "    sys.stdout.write(merged)\n"
        "elif arguments[0] == 'compose' and arguments[-4:] == ['ps', '-q', '-a', 'server']:\n"
        "    print('server-container')\n"
        "elif arguments == ['logs', 'server-container']:\n"
        "    sys.stdout.write(output)\n"
        "    sys.stderr.write(record)\n"
        "else:\n"
        "    sys.exit(f'unexpected arguments: {arguments}')\n"
    )
    program.chmod(0o755)


class OwnerCode(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="junto-server-output-")
        self.addCleanup(temporary.cleanup)
        self.directory = Path(temporary.name)
        path = f"{self.directory}{os.pathsep}{os.environ['PATH']}"
        patched = mock.patch.dict(os.environ, {"PATH": path})
        patched.start()
        self.addCleanup(patched.stop)

    def deployments(self):
        settings = {"project": "junto-test", "env_file": str(self.directory / "env")}
        yield content_driver.Deployment(argparse.Namespace(**settings, compose_file="local.yaml"))
        yield recording_driver.Deployment(argparse.Namespace(**settings, compose_file=["local.yaml"]))

    def test_the_code_is_read_when_a_log_record_is_between_the_lines_of_the_notice(self):
        fake_docker(self.directory, [FIRST])
        for deployment in self.deployments():
            with self.subTest(driver=type(deployment).__module__):
                self.assertEqual(deployment.owner_code(), FIRST)

    def test_the_code_of_the_last_start_is_read(self):
        fake_docker(self.directory, [FIRST, SECOND])
        for deployment in self.deployments():
            with self.subTest(driver=type(deployment).__module__):
                self.assertEqual(deployment.owner_code(), SECOND)


if __name__ == "__main__":
    unittest.main()
