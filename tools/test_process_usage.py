"""The reading of `footprint` and `ps` output and the summary of tools/process_usage.py."""

from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
import process_usage  # noqa: E402

FOOTPRINT = """\
Example Helper [201]: 64-bit    Footprint: 3000 B (16384 bytes per page)
     Dirty      Clean  Reclaimable    Regions    Category
      ---        ---         ---        ---    ---
     1000 B       0 B         0 B          2    IOAccelerator
      700 B       0 B         0 B          1    IOAccelerator (graphics)
     3000 B       0 B         0 B          9    TOTAL
    phys_footprint: 3072 B
    phys_footprint_peak: 9000 B
Example [100]: 64-bit    Footprint: 5000 B (16384 bytes per page)
     5000 B       0 B         0 B         20    TOTAL
    phys_footprint: 5120 B
"""


class TestReadings(unittest.TestCase):
    def test_each_process_has_its_footprint_and_its_graphics(self):
        self.assertEqual(
            process_usage.parse_footprint(FOOTPRINT),
            {201: (3072, 700), 100: (5120, 0)},
        )

    def test_the_processor_time_of_ps_is_read_in_every_form(self):
        self.assertEqual(process_usage.parse_time("0:01.50"), 1.5)
        self.assertEqual(process_usage.parse_time("2:03.25"), 123.25)
        self.assertEqual(process_usage.parse_time("1:00:00.00"), 3600.0)
        self.assertEqual(process_usage.parse_time("2-00:00:01.00"), 172801.0)

    def test_the_summary_takes_the_highest_readings_and_the_busiest_interval(self):
        samples = [
            (0.0, 100, 10, 1.0, 20),
            (5.0, 300, 30, 1.5, 25),
            (10.0, 200, 20, 3.5, 22),
        ]
        result = process_usage.summary(samples)
        self.assertEqual(result["footprint"], 300)
        self.assertEqual(result["graphics"], 30)
        self.assertEqual(result["threads"], 25)
        self.assertAlmostEqual(result["processor_average"], 25.0)
        self.assertAlmostEqual(result["processor_peak"], 40.0)


if __name__ == "__main__":
    unittest.main()
