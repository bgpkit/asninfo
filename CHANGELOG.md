# Changelog

All notable changes to this project will be documented in this file.

## v0.4.3 - 2025-10-29

* update `bgpkit-commons`, `oneio`, and `peeringdb-rs` to resolve potential rustls provider issue
* remove unnecessary rustls dependency and auxiliary code

## v0.4.2 - 2025-10-29

* update `bgpkit-commons` to `v0.9.5` to address CAIDA as2org dataset loading issue

## v0.4.1 YANKED - 2025-10-29

* add `Cargo.toml` to docker image for reproducible builds
* update dependencies

## v0.4.0 - 2025-10-01

### Added

- New HTTP API server: `asninfo serve` subcommand with CORS enabled and background refresh.
- Lookup endpoints: `GET /lookup` and `POST /lookup` with optional `legacy=true` response.
- Health endpoint: `GET /health`.
- Environment variable `ASNINFO_MAX_ASNS` to cap ASNs per request (default 100).
- Request logging for HTTP server; requests to `/health` are excluded to reduce noise.

### Changed

- Documentation: README expanded with commands, HTTP API usage, Docker examples, and environment variables.
- Clarified that CSV exports always use the simplified schema; `--simplified` flag implies reduced dataset (skips
  population, hegemony, and PeeringDB data).
- Docker README: added examples showing how to pass a local .env using `--env-file` or by bind-mounting to
  `/asninfo/.env`.

## v0.3.4 - 2025-09-10

* update `bgpkit-commons` to `v0.9.4` to support older Rust versions
* change upload behavior: remove `--upload` CLI flag; uploads are now enabled when `ASNINFO_UPLOAD_PATH` env var is
  set (S3-compatible target)
* add optional heartbeat: when `ASNINFO_HEARTBEAT_URL` is set and upload succeeds, a request is sent to signal
  completion

## v0.3.3 - 2025-09-09

* update `bgpkit-commons` to `v0.9.3`
    * this fixes the peeringdb 403 error issue
* update `oneio` to `v0.19.0`

## v0.3.2 - 2025-06-06

* update `bgpkit-commons` to `v0.8.2`
* update `oneio` to `v0.18.2`

## v0.3.1 - 2025-06-01

* update `bgpkit-commons` to `v0.8.1`
* update `oneio` to `v0.18.1`

## v0.3.0 - 2025-05-27

* update `bgpkit-commons` to `v0.8.0` to include peeringdb asninfo

## v0.2.1 - 2025-04-04

* update `bgpkit-commons` to `v0.7.4` to resolve org name latin-1 encoding issue

## v0.2.0 - 2025-02-21

### New Features

* support exporting simplified data in CSV format

## v0.1.0 - 2025-02-21

Initial release:

* release `asninfo` binary tool that can generate ASN information
    * support export to local JSON/JSONL files
    * support upload to S3-compatible object storage systems
    * support simplified format