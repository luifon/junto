"""Native camera passthrough is limited to character device nodes and their groups."""
from pathlib import Path
import stat
from types import SimpleNamespace
import unittest
from unittest.mock import patch

import camera


class CameraDevices(unittest.TestCase):
    def test_only_video_character_devices_are_exposed_without_mknod_permission(self):
        nodes = [Path("/dev/video0"), Path("/dev/video1"), Path("/dev/video-note"), Path("/dev/video2")]
        info = {
            nodes[0]: SimpleNamespace(st_mode=stat.S_IFCHR, st_gid=44),
            nodes[1]: SimpleNamespace(st_mode=stat.S_IFLNK, st_gid=0),
            nodes[3]: SimpleNamespace(st_mode=stat.S_IFREG, st_gid=0),
        }
        with patch.object(Path, "lstat", lambda path: info[path]):
            result = camera.command(["docker", "run", "image", "capture"], nodes)
        self.assertEqual(result, ["docker", "run", "--device", "/dev/video0:/dev/video0:rw",
                                 "-e", "JUNTO_TEST_CAMERA_DEVICE=1", "-e", "JUNTO_LINUX_CAMERA_GROUPS=44",
                                 "image", "capture"])

    def test_absent_devices_and_other_commands_are_refused(self):
        with self.assertRaisesRegex(ValueError, "no V4L2"):
            camera.command(["docker", "run", "image"], [])
        with self.assertRaisesRegex(ValueError, "docker run"):
            camera.command(["docker", "exec", "container"], [])


if __name__ == "__main__":
    unittest.main()
