"""Emit the SQL needed to bring D1's stations table in line with stations.json.

Only rows that are new, changed or removed are written. D1 bills rows_written
per row per index, so rewriting the whole table costs hundreds of thousands
of rows for what is usually a change of a few dozen stations.
"""

import json
import sys

BATCH = 500
FIELDS = ("id", "name", "lat", "lon", "mode")


def load_d1(path):
    with open(path) as f:
        payload = json.load(f)
    rows = payload[0]["results"]
    return {r["id"]: tuple(r[k] for k in FIELDS) for r in rows}


def load_json(path):
    with open(path) as f:
        stations = json.load(f)
    return {s["id"]: tuple(s[k] for k in FIELDS) for s in stations}


def quote(value):
    return "'" + str(value).replace("'", "''") + "'"


def values(row):
    id_, name, lat, lon, mode = row
    return f"({quote(id_)},{quote(name)},{lat},{lon},{quote(mode)})"


def chunks(items):
    for i in range(0, len(items), BATCH):
        yield items[i : i + BATCH]


def main(current_path, target_path):
    current = load_d1(current_path)
    target = load_json(target_path)

    upserts = [row for id_, row in target.items() if current.get(id_) != row]
    removed = [id_ for id_ in current if id_ not in target]

    for batch in chunks(upserts):
        print(
            "INSERT OR REPLACE INTO stations (id, name, lat, lon, mode) VALUES "
            + ",".join(values(r) for r in batch)
            + ";"
        )
    for batch in chunks(removed):
        print("DELETE FROM stations WHERE id IN (" + ",".join(quote(i) for i in batch) + ");")

    print(f"{len(upserts)} upserts, {len(removed)} deletes", file=sys.stderr)


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
