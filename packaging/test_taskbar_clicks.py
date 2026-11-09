import unittest

from taskbar_clicks import Observation, alternates, running_buttons, verdict


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
        self.assertIn("possible cause", result)
        self.assertNotIn("third press", result)

    def test_the_report_with_two_buttons_names_the_window_properties(self):
        result = verdict([seen(True, buttons=2), seen(True, buttons=2), seen(False, True, True)])
        self.assertIn("more than one taskbar button", result)

    def test_the_report_without_an_explanation_asks_for_the_messages(self):
        result = verdict([seen(True), seen(True), seen(True)])
        self.assertIn("third press did not restore", result)
        self.assertIn("belonged to another program", result)
        self.assertIn("the messages that the window received decide the cause", result)

    def test_only_a_button_of_a_running_window_is_pressed_and_counted(self):
        buttons = [{"name": "Junto", "x": 1, "y": 1},
                   {"name": "Junto - 1 running window", "x": 2, "y": 1}]
        self.assertEqual(running_buttons(buttons), [buttons[1]])
        self.assertEqual(running_buttons(buttons[:1]), [])

    def test_three_cycles_toggle_the_window_with_one_button(self):
        cycles = [seen(True), seen(False, True, True)] * 3
        self.assertTrue(alternates(cycles))
        self.assertFalse(alternates(cycles[:2] + [seen(True), seen(True)]))
        self.assertFalse(alternates([seen(True, buttons=2), seen(False, True, True)]))
        self.assertFalse(alternates([]))

    def test_a_record_of_another_length_is_incomplete(self):
        self.assertTrue(verdict([seen(True)]).startswith("incomplete"))


if __name__ == "__main__":
    unittest.main()
