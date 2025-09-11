# ASN Information Data Uploader

Fetch the latest ASN information and export to JSON, JSONL, or CSV. Optionally upload to an S3-compatible target when
configured via environment variables.

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

## Usage

```shell
Usage: asninfo [OPTIONS] [PATH]

Arguments:
  [PATH]  Export data path [default: ./asninfo.jsonl]
           Format is inferred from file extension: .json, .jsonl, or .csv

Options:
  -s, --simplified  Export simplified fields (implied for .csv)
  -h, --help        Print help
  -V, --version     Print version
```

Notes

- Upload is automatic when ASNINFO_UPLOAD_PATH is set (no --upload flag).
- .env files are supported (dotenv).

## Environment variables

Required for S3/R2 upload (when ASNINFO_UPLOAD_PATH is set):

- AWS_REGION — for Cloudflare R2, use "auto"
- AWS_ENDPOINT — S3-compatible endpoint URL
- AWS_ACCESS_KEY_ID
- AWS_SECRET_ACCESS_KEY
- ASNINFO_UPLOAD_PATH — destination like s3://bucket/key or r2://bucket/key

Optional:

- ASNINFO_HEARTBEAT_URL — HTTP/HTTPS URL to request after a successful upload (used as a heartbeat)
- PEERINGDB_API_KEY — used by dependencies to access PeeringDB API (avoids rate limits)