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

## Monitor

`monitor/` is a second Worker that probes `api`, `api1` and `api2` every five minutes (health, departures freshness, nearby, formation, api/api1 parity) and pushes to an [ntfy](https://ntfy.sh) topic only when a host changes state.

```sh
cd monitor
npx wrangler kv namespace create STATE   # paste the id into wrangler.toml
npx wrangler secret put NTFY_TOPIC       # long and random, it is the password
npx wrangler secret put API_KEY
npx wrangler deploy
```

`NTFY_TOKEN` is optional, for an access-protected topic. Remove the cron trigger to roll back.

## License

MIT. See `LICENSE`.

The deployed service at api.traintime.ch and its API keys are not part of the licence. `data/stations.json` is Didok data under "Open use. Must provide the source."
