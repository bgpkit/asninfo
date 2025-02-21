# ASN Information Data Uploader

This script fetches the most up-to-date ASN information and saves as JSON or JSONL file and may upload to S3/R2 bucket
if environment variables are set.

## Usage

```shell
➜  asninfo git:(main) ✗ cargo run --release -- --help
    Finished `release` profile [optimized] target(s) in 0.08s
     Running `target/release/asninfo --help`
Usage: asninfo [OPTIONS] [PATH]

Arguments:
  [PATH]  Export data path [default: ./asninfo.jsonl]

Options:
  -u, --upload      Flag to enable upload to S3-compatible object storage
                    Required environment variables:
                    - `AWS_REGION`
                    - `AWS_ENDPOINT`
                    - `AWS_ACCESS_KEY_ID`
                    - `AWS_SECRET_ACCESS_KEY`
                    - `ASNINFO_UPLOAD_PATH`: full path with `s3` or `r2` prefix, such as `r2://spaces/broker/asninfo.jsonl`
  -s, --simplified  Simplified format, including the following fields:
                    - asn
                    - as_name
                    - org_id
                    - org_name
                    - country_code
                    - country_name
                    - data_source
      --debug       Print debug information
  -h, --help        Print help
  -V, --version     Print version
```

## S3 upload environment variables

- `AWS_REGION`
- `AWS_ENDPOINT`
- `AWS_ACCESS_KEY_ID`
- `AWS_SECRET_ACCESS_KEY`
- `ASNINFO_UPLOAD_PATH`: full path with `s3` or `r2` prefix, such as `r2://spaces/broker/asninfo.jsonl`

