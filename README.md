# traintime-api

Swiss public transit departures and nearby station lookup. Rust/Axum on Cloudflare Workers, using the [OJP 2.0 API](https://opentransportdata.swiss/).

## Setup

```sh
cp .dev.vars.example .dev.vars  # add your API_KEY and OJP_API_KEY
bash scripts/fetch-stations.sh  # download station data
npx wrangler dev
```

## Data

Station data from the [SBB Didok dataset](https://opendata.swiss/en/dataset/haltestellen-des-offentlichen-verkehrs) (Swiss Federal Office of Transport, "Open use. Must provide the source."). Updated monthly via GitHub Actions.

## Load testing

Never through `api.opentransportdata.swiss` (50/min, 20,000/day per key) or the Cloudflare edge. Run the native server with `OJP_ENDPOINT` pointed at a stub and hit the ClusterIP.

## Monitor

`monitor/` is a cron Worker that probes `api`, `api1` and `api2` and pushes state changes to an ntfy topic. A push touching `monitor/` deploys it. Secrets are `NTFY_TOPIC` and `API_KEY`.

## License

MIT, see `LICENSE`. The deployed service and its keys are not covered. Station data is Didok, "Open use. Must provide the source."
