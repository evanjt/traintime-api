"""Emit the SQL needed to bring D1's stations table in line with the station files.

The target is every data/stations*.json together, so one region's update never
removes another region's rows. Only rows that are new, changed or removed are
written. D1 bills rows_written per row per index, so rewriting the whole table
costs hundreds of thousands of rows for what is usually a change of a few dozen
stations.
"""

import json
import sys

BATCH = 500
BASE = ("id", "name", "lat", "lon", "mode")
# Nullable columns only some regions fill. Rows without them keep the original
# INSERT, so Swiss updates still apply on a database not yet migrated.
EXTRA = ("route_types", "stop_ids")
FIELDS = BASE + EXTRA


def row_of(record):
    return tuple(record.get(k) for k in FIELDS)


def load_d1(path):
    with open(path) as f:
        payload = json.load(f)
    rows = payload[0]["results"]
    return {r["id"]: row_of(r) for r in rows}


def load_json(paths):
    target = {}
    for path in paths:
        with open(path) as f:
            stations = json.load(f)
        for s in stations:
            if s["id"] in target:
                raise SystemExit(f"{path}: station {s['id']} is already in another file")
            target[s["id"]] = row_of(s)
    return target


def quote(value):
    if value is None:
        return "NULL"
    return "'" + str(value).replace("'", "''") + "'"


def values(row):
    id_, name, lat, lon, mode, *extra = row
    cells = [quote(id_), quote(name), str(lat), str(lon), quote(mode)]
    if any(e is not None for e in extra):
        cells += [quote(e) for e in extra]
    return "(" + ",".join(cells) + ")"


def chunks(items):
    for i in range(0, len(items), BATCH):
        yield items[i : i + BATCH]


def has_extra(row):
    return any(v is not None for v in row[len(BASE) :])


def main(current_path, target_paths):
    current = load_d1(current_path)
    target = load_json(target_paths)

    upserts = [row for id_, row in target.items() if current.get(id_) != row]
    removed = [id_ for id_ in current if id_ not in target]

    for columns, rows in (
        (BASE, [r for r in upserts if not has_extra(r)]),
        (FIELDS, [r for r in upserts if has_extra(r)]),
    ):
        for batch in chunks(rows):
            print(
                f"INSERT OR REPLACE INTO stations ({', '.join(columns)}) VALUES "
                + ",".join(values(r) for r in batch)
                + ";"
            )
    for batch in chunks(removed):
        print("DELETE FROM stations WHERE id IN (" + ",".join(quote(i) for i in batch) + ");")

    print(f"{len(upserts)} upserts, {len(removed)} deletes", file=sys.stderr)


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2:])
