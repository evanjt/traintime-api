#!/bin/bash
# Sync D1 to data/stations.json, writing only the rows that differ.
set -euo pipefail

cd "$(dirname "$0")/.."

DB="traintime-stations"
# Set D1_TARGET=--local to rehearse against a local database.
TARGET="${D1_TARGET:---remote}"
CURRENT=$(mktemp)
SQL=$(mktemp)
trap 'rm -f "$CURRENT" "$SQL"' EXIT

echo "Reading current rows from D1..."
npx wrangler d1 execute "$DB" "$TARGET" --json \
  --command "SELECT id, name, lat, lon, mode FROM stations" > "$CURRENT"

python3 scripts/diff_stations.py "$CURRENT" data/stations.json > "$SQL"

if [ ! -s "$SQL" ]; then
  echo "D1 already matches data/stations.json"
  exit 0
fi

npx wrangler d1 execute "$DB" "$TARGET" --file="$SQL"
