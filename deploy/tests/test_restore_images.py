"""Exercise restore image checks with real Compose rendering and simulated containers."""

import hashlib
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import unittest


DEPLOY = Path(__file__).resolve().parents[1]
DOCKER = shutil.which("docker")
SERVER_IMAGE = "junto-backup-release:1"
INIT_IMAGE = "junto-backup-init:1"


@unittest.skipUnless(DOCKER, "Docker Compose is required to render the deployment")
class RestoreImages(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="junto-restore-images-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.settings = self.root / "env"
        self.settings.write_text(
            "JUNTO_POSTGRES_PASSWORD=examplepassword\n"
            "JUNTO_LIVEKIT_API_KEY=examplekey\n"
            "JUNTO_LIVEKIT_API_SECRET=examplesecret\n"
            "JUNTO_REDIS_PASSWORD=examplepassword\n"
            "JUNTO_SERVER_HOST=chat.example.test\n"
            "JUNTO_LIVEKIT_HOST=call.example.test\n"
            "JUNTO_TURN_HOST=turn.example.test\n"
            "JUNTO_PUBLIC_IP=192.0.2.1\n"
            "JUNTO_ACME_EMAIL=admin@example.test\n"
            f"JUNTO_MODELS_DIR={self.root}\n"
            f"JUNTO_SERVER_IMAGE={SERVER_IMAGE}\n"
            f"JUNTO_INIT_IMAGE={INIT_IMAGE}\n"
        )
        self.backup = self.root / "backup"
        self.backup.mkdir()
        (self.backup / "database.dump").write_bytes(b"database")
        (self.backup / "env").write_bytes(self.settings.read_bytes())
        (self.backup / "schema").write_text("schema 1\n")
        with tarfile.open(self.backup / "files.tar", "w") as archive:
            for name in ("attachments", "recordings"):
                member = tarfile.TarInfo(name)
                member.type = tarfile.DIRTYPE
                archive.addfile(member, io.BytesIO())
        self.checksums()
        self.log = self.root / "commands.jsonl"
        executable = self.root / "docker"
        executable.write_text('''#!/usr/bin/env python3
import json
import os
import subprocess
import sys

args = sys.argv[1:]
with open(os.environ["RESTORE_COMMANDS"], "a") as log:
    log.write(json.dumps(args) + "\\n")
if args[0] == "compose" and "config" in args:
    result = subprocess.run([os.environ["REAL_DOCKER"], *args])
    sys.exit(result.returncode)
if args[:2] == ["image", "inspect"]:
    image = args[-1]
    available = json.loads(os.environ["RESTORE_AVAILABLE_IMAGES"])
    sys.exit(0 if image in available else 1)
if args[0] == "run" and args[-1] == "schema-version":
    print("schema 1")
elif args[0] == "inspect":
    print("junto-restore-second")
elif args[0] == "compose" and "ps" in args:
    print("postgres-container")
if args[0] == "compose" and ("exec" in args or "run" in args):
    sys.stdin.buffer.read()
''')
        executable.chmod(0o755)
        self.environment = dict(os.environ, PATH=f"{self.root}:{os.environ['PATH']}",
                                REAL_DOCKER=DOCKER, RESTORE_COMMANDS=str(self.log),
                                JUNTO_ENV_FILE=str(self.settings),
                                COMPOSE_PROJECT_NAME="junto-restore-second")
        self.environment.pop("JUNTO_SERVER_IMAGE", None)
        self.environment.pop("JUNTO_INIT_IMAGE", None)
        rendered = subprocess.run(
            [DOCKER, "compose", "--env-file", str(self.settings), "-f", "local.yaml",
             "config", "--format", "json"], cwd=DEPLOY, env=self.environment,
            capture_output=True, text=True, check=True,
        )
        services = json.loads(rendered.stdout)["services"]
        self.images = {SERVER_IMAGE, INIT_IMAGE, services["postgres"]["image"], services["recorder"]["image"]}

    def checksums(self):
        files = sorted(p for p in self.backup.iterdir() if p.name != "SHA256SUMS")
        (self.backup / "SHA256SUMS").write_text("".join(
            f"{hashlib.sha256(path.read_bytes()).hexdigest()}  {path.name}\n" for path in files
        ))

    def restore(self, missing=None):
        self.environment["RESTORE_AVAILABLE_IMAGES"] = json.dumps(sorted(self.images - {missing}))
        result = subprocess.run(
            ["sh", str(DEPLOY / "restore.sh"), "local.yaml", str(self.backup)],
            env=self.environment, capture_output=True, text=True,
        )
        commands = [json.loads(line) for line in self.log.read_text().splitlines()]
        return result, commands

    def assert_no_changes(self, commands):
        self.assertTrue(all(command[:2] == ["image", "inspect"] or
                            (command[0] == "compose" and "config" in command)
                            for command in commands), commands)

    def test_second_project_uses_prebuilt_images_without_builds(self):
        result, commands = self.restore()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("restore: done", result.stdout)
        self.assertFalse(any("build" in command for command in commands), commands)
        schema = next(command for command in commands if command[0] == "run")
        self.assertEqual(schema[-2:], [SERVER_IMAGE, "schema-version"])
        self.assertIn("--pull", schema)
        inspected = {command[-1] for command in commands if command[:2] == ["image", "inspect"]}
        self.assertEqual(inspected, self.images)
        for command in commands:
            if command[0] == "compose" and ("run" in command or "up" in command):
                self.assertIn("restore-images.yaml", command)
                pull = command.index("--pull")
                self.assertEqual(command[pull + 1], "never")
                self.assertIn("--no-deps" if "run" in command else "--no-build", command)

    def test_image_id_from_original_project_is_used(self):
        image_id = "sha256:" + "a" * 64
        self.settings.write_text(self.settings.read_text().replace(SERVER_IMAGE, image_id))
        self.images = (self.images - {SERVER_IMAGE}) | {image_id}
        result, commands = self.restore()
        self.assertEqual(result.returncode, 0, result.stderr)
        schema = next(command for command in commands if command[0] == "run")
        self.assertEqual(schema[-2:], [image_id, "schema-version"])

    def test_missing_image_fails_before_any_container_command(self):
        for missing in sorted(self.images):
            with self.subTest(image=missing):
                self.log.unlink(missing_ok=True)
                result, commands = self.restore(missing)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(missing, result.stderr)
                self.assertIn("Nothing was changed", result.stderr)
                self.assertRegex(result.stderr, r"docker (compose .* build|pull )")
                self.assert_no_changes(commands)

    def test_legacy_backup_also_requires_every_image(self):
        (self.backup / "schema").unlink()
        self.checksums()
        result, commands = self.restore(SERVER_IMAGE)
        self.assertNotEqual(result.returncode, 0)
        self.assert_no_changes(commands)

    def test_explicit_image_setting_is_required(self):
        settings = self.settings.read_text()
        for setting in ("JUNTO_SERVER_IMAGE", "JUNTO_INIT_IMAGE"):
            with self.subTest(setting=setting):
                self.log.unlink(missing_ok=True)
                self.settings.write_text("\n".join(
                    line for line in settings.splitlines() if not line.startswith(f"{setting}=")
                ) + "\n")
                result, commands = self.restore()
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(setting, result.stderr)
                self.assert_no_changes(commands)

    def test_restore_configuration_cannot_build_init_or_server(self):
        for form in ("local.yaml", "one-host.yaml", "two-hosts-data.yaml"):
            with self.subTest(form=form):
                result = subprocess.run(
                    [DOCKER, "compose", "--env-file", str(self.settings), "-f", form,
                     "-f", "restore-images.yaml", "config", "--format", "json"],
                    cwd=DEPLOY, env=self.environment, capture_output=True, text=True,
                )
                self.assertEqual(result.returncode, 0, result.stderr)
                services = json.loads(result.stdout)["services"]
                for name in ("init", "server"):
                    self.assertNotIn("build", services[name])
                    self.assertEqual(services[name]["image"], INIT_IMAGE if name == "init" else SERVER_IMAGE)


if __name__ == "__main__":
    unittest.main()
