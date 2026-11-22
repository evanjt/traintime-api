# traintime-api

Swiss public transit departures and nearby station lookup. Rust/Axum on Cloudflare Workers, using the [OJP 2.0 API](https://opentransportdata.swiss/).

## Setup

```sh
cp .dev.vars.example .dev.vars  # add your API_KEY and OJP_API_KEY
bash scripts/fetch-stations.sh  # download station data
npx wrangler dev
```

## Data

Station data from the [SBB Didok dataset](https://opendata.swiss/en/dataset/haltestellen-des-offentlichen-verkehrs) (Swiss Federal Office of Transport, "Open use. Must provide the source."). Updated monthly via GitHub Actions into `data/stations.json`.

Each other region keeps its own `data/stations-<region>.json`, so the Didok update never touches it. `scripts/fetch-stations-au-vic.sh` writes the Victorian file from the [GTFS Schedule](https://opendata.transport.vic.gov.au/dataset/gtfs-schedule) (CC BY 4.0, Public Transport Victoria). The native server, `seed_d1.sh` and the D1 seed workflow read every `data/stations*.json`. Apply `migrations/m20261122_203316_add_station_routes.sql` to D1 before seeding rows that carry `route_types`.

Script tests: `cd scripts && python3 -m unittest`.

## Load testing

Never through `api.opentransportdata.swiss` (50/min, 20,000/day per key) or the Cloudflare edge. Run the native server with `OJP_ENDPOINT` pointed at a stub and hit the ClusterIP.

The Worker reads the same variable, for local bursts only:

```sh
npx wrangler dev --local --var OJP_ENDPOINT:http://127.0.0.1:8799/ojp20
```

## Monitor

`monitor/` is a cron Worker that probes `api`, `api1` and `api2` and pushes state changes to an ntfy topic. A push touching `monitor/` deploys it. Secrets are `NTFY_TOPIC` and `API_KEY`.

## License

MIT, see `LICENSE`. The deployed service and its keys are not covered. Station data is Didok, "Open use. Must provide the source.", and Public Transport Victoria GTFS, CC BY 4.0. See `data/ATTRIBUTION`.
