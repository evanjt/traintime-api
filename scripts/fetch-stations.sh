#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
OUTPUT="$SCRIPT_DIR/../src/data/stations.json"

URL="https://data.sbb.ch/api/explore/v2.1/catalog/datasets/dienststellen-gemass-opentransportdataswiss/exports/json?select=number,designationofficial,geopos,meansoftransport,stoppoint&where=stoppoint%20%3D%20%22true%22"

echo "Downloading Didok station dataset..."
RAW=$(curl -sf "$URL")
echo "  Got $(echo "$RAW" | jq length) raw records"

mkdir -p "$(dirname "$OUTPUT")"

# Transform, sort, and write one station per line
echo "$RAW" | jq '
  def map_mode:
    if . == null then "train"
    elif (split("|") | map(ascii_upcase)) as $t |
      ($t | any(. == "TRAM" or . == "METRO")) then "tram"
    elif (split("|") | map(ascii_upcase) | any(. == "BUS")) then "bus"
    elif (split("|") | map(ascii_upcase) | any(. == "TRAIN")) then "train"
    elif (split("|") | map(ascii_upcase) | any(. == "BOAT" or . == "CABLE_CAR" or . == "CABLE_RAILWAY" or . == "CHAIRLIFT" or . == "RACK_RAILWAY" or . == "ELEVATOR")) then "special"
    else "train"
    end;

  [ .[]
    | select(.geopos != null)
    | {
        id: (.number | tostring),
        name: .designationofficial,
        lat: ((.geopos.lat * 100000 | round) / 100000),
        lon: ((.geopos.lon * 100000 | round) / 100000),
        mode: (.meansoftransport | map_mode)
      }
  ]
  | sort_by(.id | tonumber)
  | map(tojson)
  | "[\n" + join(",\n") + "\n]"
' -r > "$OUTPUT"

COUNT=$(wc -l < "$OUTPUT")
SIZE=$(du -h "$OUTPUT" | cut -f1)
echo "Wrote $OUTPUT ($SIZE, $((COUNT - 2)) stations)"
