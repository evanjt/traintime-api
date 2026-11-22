import io
import json
import os
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout

import diff_stations

SWISS = {"id": "8500001", "name": "Testdorf", "lat": 47.1, "lon": 7.1, "mode": "train"}
SWISS_MOVED = dict(SWISS, lat=47.2)
LAKESIDE = {
    "id": "au-vic-9001",
    "name": "Lakeside Station",
    "lat": -37.8,
    "lon": 145.0,
    "mode": "train",
    "route_types": "0,3",
    "stop_ids": "9001",
}


class DiffStationsTest(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.addCleanup(self.dir.cleanup)

    def write(self, name, payload):
        path = os.path.join(self.dir.name, name)
        with open(path, "w") as f:
            json.dump(payload, f)
        return path

    def d1(self, *rows):
        # D1 returns every column, with NULL for the ones a Swiss row lacks.
        full = [dict({"route_types": None, "stop_ids": None}, **r) for r in rows]
        return self.write("current.json", [{"results": full}])

    def run_diff(self, current, *targets):
        out = io.StringIO()
        with redirect_stdout(out), redirect_stderr(io.StringIO()):
            diff_stations.main(current, list(targets))
        return out.getvalue()

    def test_row_from_another_file_is_kept_when_only_swiss_file_changed(self):
        current = self.d1(SWISS, LAKESIDE)
        swiss = self.write("stations.json", [SWISS_MOVED])
        vic = self.write("stations-au-vic.json", [LAKESIDE])

        sql = self.run_diff(current, swiss, vic)

        self.assertNotIn("DELETE", sql)
        self.assertIn("'8500001'", sql)
        self.assertNotIn("au-vic-9001", sql)

    def test_new_row_with_routes_names_the_extra_columns(self):
        current = self.d1(SWISS)
        sql = self.run_diff(
            current, self.write("stations.json", [SWISS]), self.write("stations-au-vic.json", [LAKESIDE])
        )

        self.assertIn("(id, name, lat, lon, mode, route_types, stop_ids)", sql)
        self.assertIn("'0,3','9001'", sql)

    def test_swiss_rows_use_the_original_columns(self):
        sql = self.run_diff(self.d1(), self.write("stations.json", [SWISS]))

        self.assertIn("(id, name, lat, lon, mode) VALUES", sql)
        self.assertNotIn("route_types", sql)

    def test_row_gone_from_every_file_is_deleted(self):
        sql = self.run_diff(self.d1(SWISS, LAKESIDE), self.write("stations.json", [SWISS]))

        self.assertIn("DELETE FROM stations WHERE id IN ('au-vic-9001');", sql)


if __name__ == "__main__":
    unittest.main()
