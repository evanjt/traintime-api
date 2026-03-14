#!/bin/bash
set -euo pipefail

BASE_URL="${BASE_URL:-https://api.traintime.ch}"

# Resolve API key: env var > .dev.vars
if [ -z "${API_KEY:-}" ]; then
  DEV_VARS="$(cd "$(dirname "$0")/.." && pwd)/.dev.vars"
  if [ -f "$DEV_VARS" ]; then
    API_KEY="$(grep '^API_KEY=' "$DEV_VARS" | cut -d= -f2)"
  fi
fi

if [ -z "${API_KEY:-}" ]; then
  echo "Error: API_KEY env var not set and .dev.vars not found"
  exit 1
fi

# Colors
GREEN='\033[0;32m'
RED='\033[0;31m'
NC='\033[0m'

PASS=0
FAIL=0

pass() {
  echo -e "  ${GREEN}PASS${NC} $1"
  PASS=$((PASS + 1))
}

fail() {
  echo -e "  ${RED}FAIL${NC} $1: $2"
  FAIL=$((FAIL + 1))
}

# Helper: curl with API key
api() {
  curl -sf -H "X-API-Key: $API_KEY" "$BASE_URL$1"
}

# Helper: curl without following redirects, capture status
status_code() {
  curl -s -o /dev/null -w '%{http_code}' "$@"
}

echo "Running e2e tests against $BASE_URL"
echo ""

# -------------------------------------------------------------------
echo "1. Health check"
HEALTH=$(curl -sf "$BASE_URL/health")
if echo "$HEALTH" | jq -e '.status == "ok"' > /dev/null 2>&1; then
  pass "GET /health returns status ok"
else
  fail "GET /health" "expected status ok, got: $HEALTH"
fi

# -------------------------------------------------------------------
echo "2. Auth denied without key"
CODE=$(status_code "$BASE_URL/v1/nearby")
if [ "$CODE" = "401" ]; then
  pass "GET /v1/nearby without key returns 401"
else
  fail "Auth denied" "expected 401, got $CODE"
fi

# -------------------------------------------------------------------
echo "3. Nearby Lucerne (boats)"
LU=$(api "/v1/nearby?lat=47.0502&lon=8.3102")

for key in train bus tram special; do
  if echo "$LU" | jq -e "has(\"$key\")" > /dev/null 2>&1; then
    pass "Lucerne response has '$key' key"
  else
    fail "Lucerne keys" "missing '$key'"
  fi
done

# special array non-empty with station 8508492
if echo "$LU" | jq -e '.special | length > 0' > /dev/null 2>&1; then
  pass "Lucerne special array non-empty"
else
  fail "Lucerne special" "array is empty"
fi

if echo "$LU" | jq -e '.special[] | select(.id == "8508492")' > /dev/null 2>&1; then
  pass "Lucerne special contains station 8508492 (Bahnhofquai)"
else
  fail "Lucerne special" "station 8508492 not found"
fi

DIST=$(echo "$LU" | jq '[.special[] | select(.id == "8508492")] | .[0].dist')
if [ "$DIST" -ge 50 ] && [ "$DIST" -le 500 ] 2>/dev/null; then
  pass "Lucerne Bahnhofquai dist=${DIST}m (50-500m)"
else
  fail "Lucerne dist" "expected 50-500m, got ${DIST}m"
fi

# Closest station in each non-empty group has departures
for key in train bus tram special; do
  LEN=$(echo "$LU" | jq ".$key | length")
  if [ "$LEN" -gt 0 ] 2>/dev/null; then
    if echo "$LU" | jq -e ".$key[0].departures | type == \"array\"" > /dev/null 2>&1; then
      pass "Lucerne $key[0] has departures array"
    else
      fail "Lucerne departures" "$key[0] missing departures"
    fi
  fi
done

# -------------------------------------------------------------------
echo "4. Nearby Bern Marzili (funicular)"
MARZILI=$(api "/v1/nearby?lat=46.946&lon=7.442")

if echo "$MARZILI" | jq -e '.special | length > 0' > /dev/null 2>&1; then
  pass "Marzili special array non-empty"
else
  fail "Marzili special" "array is empty"
fi

if echo "$MARZILI" | jq -e '.special[] | select(.name | test("Marzili"; "i"))' > /dev/null 2>&1; then
  pass "Marzili special contains station matching 'Marzili'"
else
  fail "Marzili special" "no station matching 'Marzili'"
fi

MDIST=$(echo "$MARZILI" | jq '[.special[] | select(.name | test("Marzili"; "i"))] | .[0].dist')
if [ "$MDIST" -le 1000 ] 2>/dev/null; then
  pass "Marzili dist=${MDIST}m (<=1000m)"
else
  fail "Marzili dist" "expected <=1000m, got ${MDIST}m"
fi

# -------------------------------------------------------------------
echo "5. Nearby Wabern/Gurten (funicular)"
GURTEN=$(api "/v1/nearby?lat=46.928&lon=7.446")

if echo "$GURTEN" | jq -e '.special | length > 0' > /dev/null 2>&1; then
  pass "Wabern special array non-empty"
else
  fail "Wabern special" "array is empty"
fi

if echo "$GURTEN" | jq -e '.special[] | select(.name | test("Gurten"; "i"))' > /dev/null 2>&1; then
  pass "Wabern special contains station matching 'Gurten'"
else
  fail "Wabern special" "no station matching 'Gurten'"
fi

# -------------------------------------------------------------------
echo "6. Departures for Zurich HB (train)"
ZHB=$(api "/v1/departures?id=8503000&limit=5")

if echo "$ZHB" | jq -e '.departures | type == "array" and length > 0' > /dev/null 2>&1; then
  pass "Zurich HB departures non-empty"
else
  fail "Zurich HB departures" "empty or missing"
fi

NOW=$(date +%s)
VALID=true
while IFS= read -r dep; do
  TO=$(echo "$dep" | jq -r '.to')
  DEPARTURE=$(echo "$dep" | jq '.departure')
  CATEGORY=$(echo "$dep" | jq -r '.category')
  NUMBER=$(echo "$dep" | jq -r '.number')
  PLATFORM=$(echo "$dep" | jq -r '.platform')
  PCHANGED=$(echo "$dep" | jq '.platformChanged')
  DELAY=$(echo "$dep" | jq '.delay')

  # to: non-empty string
  if [ -z "$TO" ] || [ "$TO" = "null" ]; then VALID=false; fi
  # departure: number, reasonable unix timestamp (within 10 min of now)
  PAST_CUTOFF=$((NOW - 600))
  if ! echo "$DEPARTURE" | grep -qE '^[0-9]+$' || [ "$DEPARTURE" -lt "$PAST_CUTOFF" ] 2>/dev/null; then VALID=false; fi
  # category: string
  if [ -z "$CATEGORY" ] || [ "$CATEGORY" = "null" ]; then VALID=false; fi
  # number: string
  if [ "$NUMBER" = "null" ]; then VALID=false; fi
  # platform: string
  if [ "$PLATFORM" = "null" ]; then VALID=false; fi
  # platformChanged: boolean
  if [ "$PCHANGED" != "true" ] && [ "$PCHANGED" != "false" ]; then VALID=false; fi
  # delay: number or null
  if [ "$DELAY" != "null" ] && ! echo "$DELAY" | grep -qE '^-?[0-9]+$'; then VALID=false; fi
done < <(echo "$ZHB" | jq -c '.departures[]')

if $VALID; then
  pass "Zurich HB departure shape validated"
else
  fail "Zurich HB departure shape" "one or more fields invalid"
fi

# -------------------------------------------------------------------
echo "7. Departures for Luzern Bahnhofquai (special/boat)"
BOAT=$(api "/v1/departures?id=8508492&limit=5")

if echo "$BOAT" | jq -e '.departures | type == "array"' > /dev/null 2>&1; then
  pass "Bahnhofquai departures has correct shape"
else
  fail "Bahnhofquai departures" "missing departures array"
fi

# -------------------------------------------------------------------
echo "8. Error: missing params"
CODE=$(status_code -H "X-API-Key: $API_KEY" "$BASE_URL/v1/nearby")
if [ "$CODE" = "400" ]; then
  pass "GET /v1/nearby without lat/lon returns 400"
else
  fail "Missing params nearby" "expected 400, got $CODE"
fi

CODE=$(status_code -H "X-API-Key: $API_KEY" "$BASE_URL/v1/departures")
if [ "$CODE" = "400" ]; then
  pass "GET /v1/departures without id returns 400"
else
  fail "Missing params departures" "expected 400, got $CODE"
fi

# -------------------------------------------------------------------
echo "9. Error: unknown route"
CODE=$(status_code -H "X-API-Key: $API_KEY" "$BASE_URL/v1/unknown")
if [ "$CODE" = "404" ]; then
  pass "GET /v1/unknown returns 404"
else
  fail "Unknown route" "expected 404, got $CODE"
fi

# -------------------------------------------------------------------
echo ""
echo "Results: ${PASS} passed, ${FAIL} failed"

if [ "$FAIL" -gt 0 ]; then
  exit 1
fi
