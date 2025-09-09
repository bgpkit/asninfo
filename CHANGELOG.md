# Changelog

All notable changes to this project will be documented in this file.

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