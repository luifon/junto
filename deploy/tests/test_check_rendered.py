"""The comparison of the configuration of the media server of a form with
deploy/livekit-common.yaml (ADR 0904)."""

import unittest

from check_rendered import LIVEKIT_COMMON, livekit_problems

COMMON = LIVEKIT_COMMON.read_text(encoding="utf-8")
HOST = """port: 7880
keys:
  key: secret
rtc:
  tcp_port: 7881
  udp_port: 7882
webhook:
  api_key: key
  urls:
    - http://server:8080/internal/media/notifications
logging:
  level: info
"""


class LivekitSettings(unittest.TestCase):
    def test_the_common_settings_with_settings_of_the_host_pass(self):
        self.assertEqual(livekit_problems(HOST + COMMON), [])

    def test_comments_and_their_place_do_not_count(self):
        configuration = HOST + "# A comment.\n" + COMMON.replace("room:\n", "room:\n  # Another.\n")
        self.assertEqual(livekit_problems(configuration), [])

    def test_a_changed_value_fails(self):
        changed = COMMON.replace("cycles_required: 4", "cycles_required: 20", 1)
        self.assertNotEqual(changed, COMMON)
        self.assertEqual(livekit_problems(HOST + changed),
                         ["livekit: the section video differs from deploy/livekit-common.yaml"])

    def test_a_setting_that_only_the_form_has_fails(self):
        added = COMMON.replace("room:\n", "room:\n  empty_timeout: 300\n")
        self.assertEqual(livekit_problems(HOST + added),
                         ["livekit: the section room differs from deploy/livekit-common.yaml"])

    def test_a_missing_section_fails(self):
        without_room = COMMON.replace("room:\n", "unused:\n")
        self.assertIn("livekit: the section room differs from deploy/livekit-common.yaml",
                      livekit_problems(HOST + without_room))

    def test_a_section_that_is_neither_common_nor_of_the_host_fails(self):
        self.assertEqual(livekit_problems(HOST + COMMON + "audio:\n  active_level: 30\n"),
                         ["livekit: the sections ['audio'] are not in deploy/livekit-common.yaml "
                          "and do not depend on the host"])

    def test_a_repeated_section_fails(self):
        self.assertIn("livekit: the sections ['video'] appear more than once",
                      livekit_problems(HOST + COMMON + "video:\n  enable_dynacast: true\n"))


if __name__ == "__main__":
    unittest.main()
