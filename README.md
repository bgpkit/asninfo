# ASN Information Data Uploader

This script fetches the most up-to-date ASN information and save as JSONL file and may upload to S3/R2 bucket if 
environment variables are set.

## Usage

```shell
cargo run --release
```

## S3 upload environment variables

- `AWS_REGION`
- `AWS_ENDPOINT`
- `AWS_ACCESS_KEY_ID`
- `AWS_SECRET_ACCESS_KEY`
- `ASNINFO_UPLOAD_PATH`: full path with `s3` or `r2` prefix, such as `r2://spaces/broker/asninfo.jsonl`

