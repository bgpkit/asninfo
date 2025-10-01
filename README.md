# ASN Information Tool (exporter and HTTP API)

Export up-to-date ASN information to JSON, JSONL, or CSV files, and optionally upload to an S3-compatible target.
You can also run a lightweight HTTP API server to perform ASN info lookups.

- Export formats: JSON, JSONL, CSV (CSV uses a simplified schema)
- Optional upload to S3/R2 via environment variables (no CLI flag needed)
- HTTP API with GET/POST lookup endpoints and CORS enabled
- .env files supported via dotenv

## Install

### Using `cargo`

```bash
cargo install asninfo
```

### Using `homebrew` on macOS

```bash
brew install bgpkit/tap/asninfo
```

### Using [`cargo-binstall`](https://github.com/cargo-bins/cargo-binstall)

```bash
cargo install cargo-binstall
cargo binstall asninfo
```

## Commands

The CLI provides two subcommands: generate and serve.

```shell
asninfo generate [OPTIONS] [PATH]

Options:
  -s, --simplified  Export simplified fields (implied for .csv)

Arguments:
  [PATH]  Export data path (default: ./asninfo.jsonl)
          Format is inferred from file extension: .json, .jsonl, or .csv
```

```shell
asninfo serve [OPTIONS]

Options:
  -b, --bind <ADDR:PORT>     Bind address (default: 0.0.0.0:8080)
      --refresh-secs <SECS>  Background refresh interval in seconds (default: 21600)
      --simplified           Use simplified mode (skip heavy datasets)
```

### Examples

- Export JSONL with full fields:

```bash
asninfo generate ./asninfo.jsonl
# same as "asninfo generate"
```

- Export CSV (simplified schema is implied):

```bash
asninfo generate ./asninfo.csv
```

- Export simplified JSON (smaller payload):

```bash
asninfo generate -s ./asninfo.json
```

- Upload automatically to S3/R2 by setting environment variables:

```bash
export ASNINFO_UPLOAD_PATH="r2://my-bucket/asn/asninfo.jsonl"
export AWS_REGION="auto"                 # for Cloudflare R2
export AWS_ENDPOINT="https://<account>.r2.cloudflarestorage.com"
export AWS_ACCESS_KEY_ID=...
export AWS_SECRET_ACCESS_KEY=...

asninfo generate ./asninfo.jsonl
```

## HTTP API

Start the server:

```bash
asninfo serve --bind 0.0.0.0:8080 --refresh-secs 21600
```

- Background updater refreshes the in-memory dataset every refresh-secs seconds (minimum 3600).
- CORS is enabled for all origins.
- Simplified mode reduces memory footprint by skipping heavy datasets (population, hegemony, PeeringDB).
- The maximum number of ASNs per request is limited by the environment variable ASNINFO_MAX_ASNS (default 100).

### Endpoints

- GET /health
    - Returns status and metadata, including updatedAt timestamp.

- GET /lookup?asns=AS1,AS2,...[&legacy=true]
    - Query parameter asns is a comma-separated list of ASNs.
    - Optional legacy=true to return a legacy array of objects instead of the structured response.

- POST /lookup
    - JSON body: { "asns": [number, ...] }
    - Note: legacy=true is only supported on GET /lookup.

### Responses

Default response (full schema plus country_name):

```json
[
  {
    "asn": 13335,
    "name": "CLOUDFLARENET",
    "country": "US",
    "country_name": "United States",
    "as2org": {
      "org_id": "CLOUD14-ARIN",
      "org_name": "Cloudflare, Inc.",
      "name": "CLOUDFLARENET",
      "country": "US"
    },
    "hegemony": {
      "asn": 13335,
      "ipv4": 0.0018,
      "ipv6": 0.0084
    },
    "peeringdb": {
      "asn": 13335,
      "name": "Cloudflare",
      "aka": "",
      "name_long": "",
      "website": "https://www.cloudflare.com",
      "irr_as_set": "AS13335:AS-CLOUDFLARE"
    },
    "population": {
      "user_count": 10,
      "sample_count": 127,
      "percent_global": 0.0,
      "percent_country": 0.02
    }
  }
]
```

Note: When the server runs with --simplified, heavy datasets (population, hegemony, PeeringDB) are omitted and will be
null in responses.

Legacy response (when legacy=true) returns an array of objects compatible with the previous consumer format.

### Example requests

```bash
# GET
curl 'http://localhost:8080/lookup?asns=13335,15169'

# POST
curl -X POST 'http://localhost:8080/lookup' \
  -H 'Content-Type: application/json' \
  -d '{"asns":[13335,15169]}'
```

```json
[
  {
    "as2org": {
      "country": "US",
      "name": "CLOUDFLARENET",
      "org_id": "CLOUD14-ARIN",
      "org_name": "Cloudflare, Inc."
    },
    "asn": 13335,
    "country": "US",
    "country_name": "United States",
    "hegemony": {
      "asn": 13335,
      "ipv4": 0.0017993252336435785,
      "ipv6": 0.008380104743151566
    },
    "name": "CLOUDFLARENET",
    "peeringdb": {
      "aka": "",
      "asn": 13335,
      "irr_as_set": "AS13335:AS-CLOUDFLARE",
      "name": "Cloudflare",
      "name_long": "",
      "website": "https://www.cloudflare.com"
    },
    "population": {
      "percent_country": 0.02,
      "percent_global": 0.0,
      "sample_count": 127,
      "user_count": 10
    }
  },
  {
    "as2org": {
      "country": "US",
      "name": "GOOGLE",
      "org_id": "GOGL-ARIN",
      "org_name": "Google LLC"
    },
    "asn": 15169,
    "country": "US",
    "country_name": "United States",
    "hegemony": {
      "asn": 15169,
      "ipv4": 0.0072255134909779304,
      "ipv6": 0.002685539203529714
    },
    "name": "GOOGLE",
    "peeringdb": {
      "aka": "Google, YouTube (for Google Fiber see AS16591 record)",
      "asn": 15169,
      "irr_as_set": "RADB::AS-GOOGLE",
      "name": "Google LLC",
      "name_long": "",
      "website": "https://about.google/intl/en/"
    },
    "population": {
      "percent_country": 0.01,
      "percent_global": 0.0,
      "sample_count": 740,
      "user_count": 521
    }
  }
]
```

## CSV simplified schema

When exporting CSV (or using --simplified), the schema is:

```
asn,as_name,org_id,org_name,country_code,country_name,data_source
```

Notes:

- country_name is looked up from country_code where available.
- data_source is reserved for future use.

## Environment variables

Required for S3/R2 upload (when ASNINFO_UPLOAD_PATH is set):

- AWS_REGION — for Cloudflare R2, use "auto"
- AWS_ENDPOINT — S3-compatible endpoint URL
- AWS_ACCESS_KEY_ID
- AWS_SECRET_ACCESS_KEY
- ASNINFO_UPLOAD_PATH — destination like s3://bucket/key or r2://bucket/key

Optional:

- ASNINFO_HEARTBEAT_URL — HTTP/HTTPS URL to request after a successful upload (used as a heartbeat)
- ASNINFO_MAX_ASNS — maximum ASNs per lookup request for the HTTP API (default: 100)
- PEERINGDB_API_KEY — used by dependencies to access PeeringDB API (avoids rate limits)

.env files are supported and loaded automatically when present.

## Docker

A minimal container image can be built using the provided Dockerfile:

```bash
docker build -t asninfo .

# run generator, mounting a host directory for output
docker run --rm -v "$PWD:/out" asninfo generate /out/asninfo.jsonl

# run HTTP server on port 8080
docker run --rm -p 8080:8080 asninfo serve --bind 0.0.0.0:8080
```

### Using a local env file (.env)

The image supports environment variables for uploads and server limits. You can pass your local .env to the container
using Docker's `--env-file`:

```bash
# pass variables from ./.env to the container environment
# works for both generate and serve

docker run --rm \
  --env-file ./.env \
  -v "$PWD:/out" \
  asninfo generate /out/asninfo.jsonl

# server example with env-file
docker run --rm \
  --env-file ./.env \
  -p 8080:8080 \
  asninfo serve --bind 0.0.0.0:8080
```