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
