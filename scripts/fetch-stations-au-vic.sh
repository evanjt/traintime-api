#!/bin/bash
# Writes data/stations-au-vic.json from the Victorian GTFS schedule. Kept apart
# from stations.json so the Didok update never touches these rows.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
OUTPUT="$SCRIPT_DIR/../data/stations-au-vic.json"

URL="https://opendata.transport.vic.gov.au/dataset/3f4e292e-7f8a-4ffe-831f-1953be0fe448/resource/8af7737c-5c9e-4022-aa98-911aa04370a7/download/gtfs.zip"

ZIP=$(mktemp)
trap 'rm -f "$ZIP"' EXIT

echo "Downloading Victorian GTFS schedule..."
curl -sfL -o "$ZIP" "$URL"
echo "  Got $(du -h "$ZIP" | cut -f1)"

python3 "$SCRIPT_DIR/stations_au_vic.py" "$ZIP" "$OUTPUT"
