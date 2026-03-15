#!/bin/bash
# Seed D1 from stations.json
cd "$(dirname "$0")/.."
python3 -c "
import json
with open('data/stations.json') as f:
    stations = json.load(f)
print('DELETE FROM stations;')
# Write in batches of 500
for i in range(0, len(stations), 500):
    batch = stations[i:i+500]
    values = ','.join(
        f\"('{s['id']}','{s['name'].replace(chr(39), chr(39)+chr(39))}',{s['lat']},{s['lon']},'{s['mode']}')\"
        for s in batch
    )
    print(f'INSERT INTO stations (id, name, lat, lon, mode) VALUES {values};')
" > /tmp/seed_stations.sql
npx wrangler d1 execute traintime-stations --remote --file=/tmp/seed_stations.sql
echo "Seeded $(wc -l < /tmp/seed_stations.sql) batches"
