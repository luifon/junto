import unittest

from taskbar_clicks import Observation, verdict


def seen(minimized, client_in_front=False, main_in_front=False, buttons=1):
    return Observation(minimized=minimized, client_in_front=client_in_front,
                       main_in_front=main_in_front, buttons=buttons)


class Verdict(unittest.TestCase):
    def test_a_minimize_restore_minimize_is_not_the_report(self):
        result = verdict([seen(True), seen(False, True, True), seen(True)])
        self.assertTrue(result.startswith("not reproduced: the second press restored"), result)

    def test_a_first_press_that_does_not_minimize_is_not_the_report(self):
        result = verdict([seen(False, True, True), seen(True), seen(False)])
        self.assertTrue(result.startswith("not reproduced: the first press"), result)

    def test_the_report_with_the_main_window_still_in_front_names_the_active_button(self):
        result = verdict([seen(True, True, True), seen(True), seen(False, True, True)])
        self.assertIn("reproduced", result)
        self.assertIn("press on the active button", result)
        self.assertNotIn("third press", result)

    def test_the_report_with_two_buttons_names_the_window_properties(self):
        result = verdict([seen(True, buttons=2), seen(True, buttons=2), seen(False, True, True)])
        self.assertIn("more than one taskbar button", result)

    def test_the_report_without_an_explanation_asks_for_the_messages(self):
        result = verdict([seen(True), seen(True), seen(True)])
        self.assertIn("third press did not restore", result)
        self.assertIn("the messages of the window are needed", result)

    def test_a_record_of_another_length_is_incomplete(self):
        self.assertTrue(verdict([seen(True)]).startswith("incomplete"))


if __name__ == "__main__":
    unittest.main()
