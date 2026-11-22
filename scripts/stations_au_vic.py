"""Build data/stations-au-vic.json from the Victorian GTFS schedule zip.

Rows carry PTV Timetable API stop ids, read from each stop's `stop_url`, so
departures can be asked of PTV directly. Train stations come from the parent
stations of the metro and V/Line feeds, tram stops from the tram feed.

Usage: stations_au_vic.py GTFS_ZIP OUTPUT
"""

import csv
import io
import json
import math
import re
import sys
import zipfile

# PTV route types, which the departures call takes per station.
METRO, TRAM, VLINE = 0, 1, 3

# Folder of each feed inside the outer zip.
FEEDS = {METRO: "2", TRAM: "3", VLINE: "1"}

MODES = {METRO: "train", VLINE: "train", TRAM: "tram"}

ID_PREFIX = "au-vic-"

# Must match the Victoria region's coverage box in core/src/region.rs.
BOX = {"lat_min": -38.50, "lat_max": -37.40, "lon_min": 144.40, "lon_max": 145.70}

# Same-name tram stops closer than this are the two directions of one stop.
MERGE_METRES = 60

PTV_ID = re.compile(r"/stop/(\d+)")


def ptv_id(stop):
    match = PTV_ID.search(stop.get("stop_url") or "")
    return match.group(1) if match else None


def in_box(lat, lon):
    return BOX["lat_min"] <= lat <= BOX["lat_max"] and BOX["lon_min"] <= lon <= BOX["lon_max"]


def metres(a, b):
    mean_lat = math.radians((a[0] + b[0]) / 2)
    x = math.radians(b[1] - a[1]) * math.cos(mean_lat)
    y = math.radians(b[0] - a[0])
    return 6_371_000 * math.hypot(x, y)


def coord(stop):
    return float(stop["stop_lat"]), float(stop["stop_lon"])


def centroid(points):
    return sum(p[0] for p in points) / len(points), sum(p[1] for p in points) / len(points)


def station_name(parent_name):
    # "Glenferrie Railway Station (Hawthorn)" reads as "Glenferrie Station", as PTV names it.
    return re.sub(r" Railway Station\b.*$", " Station", parent_name)


def train_stations(feeds):
    """Parent stations keyed by PTV id, with every route type that serves them."""
    stations = {}
    for route_type in (METRO, VLINE):
        stops = feeds.get(route_type, [])
        parents = {s["stop_id"]: s for s in stops if s.get("location_type") == "1"}
        ids = {}
        for s in stops:
            pid = ptv_id(s)
            if pid and s.get("parent_station") in parents:
                ids.setdefault(s["parent_station"], pid)
        for parent_id, pid in ids.items():
            parent = parents[parent_id]
            entry = stations.setdefault(
                pid,
                {"name": station_name(parent["stop_name"]), "point": coord(parent), "types": set()},
            )
            entry["types"].add(route_type)
    return [
        (pid, e["name"], e["point"], e["types"], [pid])
        for pid, e in stations.items()
        if in_box(*e["point"])
    ]


def tram_stations(stops):
    """Tram stops by PTV id, with same-name stops within MERGE_METRES joined."""
    by_id = {}
    for s in stops:
        pid = ptv_id(s)
        if pid:
            by_id.setdefault(pid, {"name": s["stop_name"], "points": []})["points"].append(coord(s))

    ids = sorted(by_id, key=int)
    root = {pid: pid for pid in ids}

    def find(pid):
        while root[pid] != pid:
            pid = root[pid]
        return pid

    by_name = {}
    for pid in ids:
        by_name.setdefault(by_id[pid]["name"], []).append(pid)
    for group in by_name.values():
        for i, a in enumerate(group):
            for b in group[i + 1 :]:
                if metres(centroid(by_id[a]["points"]), centroid(by_id[b]["points"])) <= MERGE_METRES:
                    ra, rb = find(a), find(b)
                    if ra != rb:
                        root[max(ra, rb, key=int)] = min(ra, rb, key=int)

    clusters = {}
    for pid in ids:
        clusters.setdefault(find(pid), []).append(pid)

    out = []
    for head, members in clusters.items():
        point = centroid([p for m in members for p in by_id[m]["points"]])
        if in_box(*point):
            out.append((head, by_id[head]["name"], point, {TRAM}, members))
    return out


def stations_from_feeds(feeds):
    """Station rows from {route_type: stops.txt rows}, sorted by PTV id."""
    found = train_stations(feeds) + tram_stations(feeds.get(TRAM, []))
    rows = []
    for pid, name, (lat, lon), types, members in sorted(found, key=lambda f: int(f[0])):
        rows.append(
            {
                "id": ID_PREFIX + pid,
                "name": name,
                "lat": round(lat, 5),
                "lon": round(lon, 5),
                "mode": MODES[min(types)],
                "route_types": ",".join(str(t) for t in sorted(types)),
                "stop_ids": ",".join(members),
            }
        )
    ids = [r["id"] for r in rows]
    if len(ids) != len(set(ids)):
        raise SystemExit("a PTV id is both a train station and a tram stop")
    return rows


def read_stops(outer, folder):
    with outer.open(f"{folder}/google_transit.zip") as nested:
        with zipfile.ZipFile(io.BytesIO(nested.read())) as feed:
            with feed.open("stops.txt") as f:
                return list(csv.DictReader(io.TextIOWrapper(f, encoding="utf-8-sig")))


def main(zip_path, output):
    with zipfile.ZipFile(zip_path) as outer:
        feeds = {rt: read_stops(outer, folder) for rt, folder in FEEDS.items()}
    rows = stations_from_feeds(feeds)
    with open(output, "w", encoding="utf-8") as f:
        f.write("[\n")
        f.write(",\n".join(json.dumps(r, ensure_ascii=False, separators=(",", ":")) for r in rows))
        f.write("\n]\n")
    modes = {}
    for r in rows:
        modes[r["mode"]] = modes.get(r["mode"], 0) + 1
    print(f"Wrote {output}: " + ", ".join(f"{n} {m}" for m, n in sorted(modes.items())))


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
