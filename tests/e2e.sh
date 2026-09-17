#!/bin/bash
set -euo pipefail

# One host, or several space separated in BASE_URLS. Every host runs the whole
# suite and any failure fails the run.
BASE_URLS="${BASE_URLS:-${BASE_URL:-https://api.traintime.ch}}"

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
FAILED_HOSTS=""

pass() {
  echo -e "  ${GREEN}PASS${NC} $1"
  PASS=$((PASS + 1))
}

fail() {
  echo -e "  ${RED}FAIL${NC} $1: $2"
  FAIL=$((FAIL + 1))
}

# Helper: curl with API key
# A dead host must show up as failed checks, not abort the run under set -e.
api() {
  curl -sf -H "X-API-Key: $API_KEY" "$BASE_URL$1" || true
}

# Helper: curl without following redirects, capture status
status_code() {
  curl -s -o /dev/null -w '%{http_code}' "$@"
}

run_suite() {
  BASE_URL="$1"
  PASS=0
  FAIL=0
  echo "Running e2e tests against $BASE_URL"
  echo ""

  # -------------------------------------------------------------------
  echo "1. Health check"
  HEALTH=$(curl -sf "$BASE_URL/health" || true)
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

  # Only the default station embeds departures: the first station of the
  # first non-empty group in train, bus, tram, special order, or of the
  # requested mode's group when ?mode= is given. Clients fetch the rest
  # from /v1/departures.
  DEFAULT_KEY=""
  for key in train bus tram special; do
    if [ -z "$DEFAULT_KEY" ] && [ "$(echo "$LU" | jq ".$key | length")" -gt 0 ]; then
      DEFAULT_KEY=$key
    fi
  done
  if echo "$LU" | jq -e ".$DEFAULT_KEY[0].departures | type == \"array\"" > /dev/null 2>&1; then
    pass "Lucerne default station ($DEFAULT_KEY[0]) has departures array"
  else
    fail "Lucerne departures" "$DEFAULT_KEY[0] missing departures"
  fi

  for key in train bus tram special; do
    if [ "$key" != "$DEFAULT_KEY" ] && echo "$LU" | jq -e ".$key[0] | has(\"departures\")" > /dev/null 2>&1; then
      fail "Lucerne departures" "$key[0] embeds departures but is not the default station"
    fi
  done

  LU_SPECIAL=$(api "/v1/nearby?lat=47.0502&lon=8.3102&mode=special")
  if echo "$LU_SPECIAL" | jq -e '.special[0].departures | type == "array"' > /dev/null 2>&1; then
    pass "Lucerne mode=special embeds departures on special[0]"
  else
    fail "Lucerne mode=special" "special[0] missing departures"
  fi

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
  echo "5b. Name query with a non-ASCII letter"
  LOE=$(api "/v1/nearby?lat=47.71412&lon=7.75601&query=l%C3%B6")

  if echo "$LOE" | jq -e '.bus[] | select(.id == "1100145")' > /dev/null 2>&1; then
    pass "query=lö matches Kirchhausen (Kr LÖ)"
  else
    fail "query=lö" "station 1100145 not found: $(echo "$LOE" | jq -c '[.bus[].name]')"
  fi

  if echo "$LOE" | jq -e '[.train[],.bus[],.tram[],.special[] | .name | select(test("lö"; "i") | not)] | length == 0' > /dev/null 2>&1; then
    pass "query=lö returns only matching names"
  else
    fail "query=lö" "non-matching name returned"
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
  echo "10. Auth denied with a wrong key"
  CODE=$(status_code -H "X-API-Key: not-a-key" "$BASE_URL/v1/departures?id=8503000&limit=1")
  if [ "$CODE" = "401" ]; then
    pass "wrong X-API-Key returns 401"
  else
    fail "Wrong key" "expected 401, got $CODE"
  fi

  # -------------------------------------------------------------------
  echo "11. Version header"
  VERSION=$(curl -s -D - -o /dev/null "$BASE_URL/health" | tr -d '\r' | awk -F': ' 'tolower($1) == "x-api-version" { print $2 }')
  if [ "$VERSION" = "1" ]; then
    pass "x-api-version: 1 present"
  else
    fail "Version header" "expected x-api-version: 1, got '${VERSION}'"
  fi

  # -------------------------------------------------------------------
  # Proves a key rollout landed on this host before an app ships with it.
  if [ -n "${EXTRA_API_KEYS:-}" ]; then
    echo "12. Extra keys"
    IFS=',' read -ra KEYS <<< "$EXTRA_API_KEYS"
    for key in "${KEYS[@]}"; do
      CODE=$(status_code -H "X-API-Key: $key" "$BASE_URL/v1/departures?id=8503000&limit=1")
      if [ "$CODE" = "200" ]; then
        pass "key $(echo "$key" | cut -c1-4)… returns 200"
      else
        fail "Extra key $(echo "$key" | cut -c1-4)…" "expected 200, got $CODE"
      fi
    done
  fi

  echo ""
  echo "$BASE_URL: ${PASS} passed, ${FAIL} failed"
  echo ""
  if [ "$FAIL" -gt 0 ]; then
    FAILED_HOSTS="$FAILED_HOSTS $BASE_URL"
  fi
}

for host in $BASE_URLS; do
  run_suite "$host"
done

if [ -n "$FAILED_HOSTS" ]; then
  echo "Failed:$FAILED_HOSTS"
  exit 1
fi
