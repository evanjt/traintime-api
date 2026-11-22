import unittest

from stations_au_vic import METRO, TRAM, VLINE, stations_from_feeds


def url(stop):
    return f"https://transport.vic.gov.au/stop/{stop}/?utm_source=x"


def row(stop_id, name, lat, lon, stop_url="", location_type="", parent=""):
    return {
        "stop_id": stop_id,
        "stop_name": name,
        "stop_lat": str(lat),
        "stop_lon": str(lon),
        "stop_url": stop_url,
        "location_type": location_type,
        "parent_station": parent,
    }


METRO_STOPS = [
    row("x:rail:OAK", "Oakfield Railway Station", -37.80000, 145.00000, location_type="1"),
    row("501", "Oakfield Station", -37.80001, 145.00002, url(9001), parent="x:rail:OAK"),
    row("502", "Oakfield Station", -37.80003, 145.00004, url(9001), parent="x:rail:OAK"),
    row("503", "Oakfield Entrance", -37.80002, 145.00001, location_type="2", parent="x:rail:OAK"),
    row("x:rail:FAR", "Faraway Railway Station", -36.90000, 146.10000, location_type="1"),
    row("601", "Faraway Station", -36.90000, 146.10000, url(9002), parent="x:rail:FAR"),
]

VLINE_STOPS = [
    row("x:rail:OAK", "Oakfield Railway Station", -37.80000, 145.00000, location_type="1"),
    row("701", "Oakfield Station", -37.80001, 145.00002, url(9001), parent="x:rail:OAK"),
]

TRAM_STOPS = [
    # One PTV id shared by both directions.
    row("801", "Elm St/Main Rd #12", -37.81000, 145.01000, url(9101)),
    row("802", "Elm St/Main Rd #12", -37.81020, 145.01020, url(9101)),
    # One PTV id per direction, 30 m apart: one station with both ids.
    row("811", "Birch St/Main Rd #14", -37.81500, 145.01500, url(9112)),
    row("812", "Birch St/Main Rd #14", -37.81527, 145.01500, url(9111)),
    # Same name but far apart: two stations.
    row("821", "Ash Pde/Lake Rd #3", -37.82000, 145.02000, url(9121)),
    row("822", "Ash Pde/Lake Rd #3", -37.83000, 145.02000, url(9122)),
]


class StationsFromFeedsTest(unittest.TestCase):
    def setUp(self):
        self.rows = stations_from_feeds({METRO: METRO_STOPS, VLINE: VLINE_STOPS, TRAM: TRAM_STOPS})
        self.by_id = {r["id"]: r for r in self.rows}

    def test_parent_station_is_one_train_row_with_both_route_types(self):
        self.assertEqual(
            self.by_id["au-vic-9001"],
            {
                "id": "au-vic-9001",
                "name": "Oakfield Station",
                "lat": -37.8,
                "lon": 145.0,
                "mode": "train",
                "route_types": "0,3",
                "stop_ids": "9001",
            },
        )

    def test_station_outside_the_box_is_dropped(self):
        self.assertNotIn("au-vic-9002", self.by_id)

    def test_tram_stop_sharing_one_id_is_one_row(self):
        row = self.by_id["au-vic-9101"]
        self.assertEqual(row["mode"], "tram")
        self.assertEqual(row["route_types"], "1")
        self.assertEqual(row["stop_ids"], "9101")
        self.assertEqual((row["lat"], row["lon"]), (-37.8101, 145.0101))

    def test_same_name_tram_pair_within_60m_merges_keeping_both_ids(self):
        self.assertNotIn("au-vic-9112", self.by_id)
        row = self.by_id["au-vic-9111"]
        self.assertEqual(row["name"], "Birch St/Main Rd #14")
        self.assertEqual(row["stop_ids"], "9111,9112")

    def test_same_name_tram_stops_far_apart_stay_apart(self):
        self.assertIn("au-vic-9121", self.by_id)
        self.assertIn("au-vic-9122", self.by_id)

    def test_rows_are_sorted_by_id(self):
        ids = [r["id"] for r in self.rows]
        self.assertEqual(ids, sorted(ids, key=lambda i: int(i.rsplit("-", 1)[1])))
        self.assertEqual(len(ids), 5)


if __name__ == "__main__":
    unittest.main()
