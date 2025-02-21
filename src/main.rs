//! Exporting ASN information to file and upload to S3 bucket
//!
//! Supported export formats:
//! 1. JSON
//! 2. JSONL
//!
//! Required environment variables for uploading to S3 bucket:
//!
//! - `AWS_REGION`
//! - `AWS_ENDPOINT`
//! - `AWS_ACCESS_KEY_ID`
//! - `AWS_SECRET_ACCESS_KEY`
//! - `ASNINFO_UPLOAD_PATH`: full path with `s3` or `r2` prefix, such as `r2://spaces/broker/asninfo.jsonl`
//!
//! For Cloudflare R2 destination, `AWS_REGION` should be `auto`.

use bgpkit_commons::asinfo::AsInfo;
use clap::Parser;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tracing::{error, info};

#[derive(Parser)]
#[clap(author, version, about, long_about = None)]
#[clap(propagate_version = true)]
struct Cli {
    /// Export data path
    #[clap(default_value = "./asninfo.jsonl")]
    path: String,

    /// Flag to enable upload to S3-compatible object storage
    #[clap(short, long)]
    upload: bool,

    /// Simplified format
    #[clap(short, long)]
    simplified: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AsInfoSimplified {
    pub asn: u32,
    pub as_name: String,
    pub org_id: String,
    pub org_name: String,
    pub country_code: String,
    pub country_name: String,
    pub data_source: String,
}

impl From<&AsInfo> for AsInfoSimplified {
    fn from(value: &AsInfo) -> Self {
        let (org_id, org_name) = match &value.as2org {
            None => ("".to_string(), "".to_string()),
            Some(v) => (v.org_id.clone(), v.org_name.clone()),
        };

        AsInfoSimplified {
            asn: value.asn,
            as_name: value.name.clone(),
            org_id,
            org_name,
            country_code: value.country.clone(),
            country_name: "".to_string(),
            data_source: "".to_string(),
        }
    }
}

fn main() {
    tracing_subscriber::fmt().with_ansi(false).init();
    let cli = Cli::parse();

    dotenvy::dotenv().ok();

    let load_population = !cli.simplified;
    let load_hegemony = !cli.simplified;

    info!("loading asn info data ...");
    let mut commons = bgpkit_commons::BgpkitCommons::new();
    commons
        .load_asinfo(true, load_population, load_hegemony)
        .unwrap();
    commons.load_countries().unwrap();
    let as_info_map = commons.asinfo_all().unwrap();

    let is_jsonl = cli.path.contains(".jsonl");
    info!(
        "export format: {}",
        match is_jsonl {
            true => "jsonl",
            false => "json",
        }
    );

    info!("writing asn info data to '{}' ...", &cli.path);
    let mut writer = oneio::get_writer(&cli.path).unwrap();
    let mut info_vec = as_info_map.values().collect::<Vec<_>>();
    info_vec.sort_by(|a, b| a.asn.cmp(&b.asn));
    let values_vec: Vec<Value> = match cli.simplified {
        false => info_vec.into_iter().map(|v| json!(v)).collect(),
        true => info_vec
            .into_iter()
            .map(|v| {
                let mut info = AsInfoSimplified::from(v);
                if let Ok(opt_name) = commons.country_by_code(&info.country_code) {
                    if let Some(name) = opt_name {
                        info.country_name = name.name
                    }
                }
                json!(info)
            })
            .collect(),
    };
    if is_jsonl {
        for as_info in values_vec {
            writeln!(writer, "{}", serde_json::to_string(&as_info).unwrap()).unwrap();
        }
    } else {
        writeln!(writer, "{}", serde_json::to_string(&values_vec).unwrap()).unwrap();
    }
    drop(writer);

    if cli.upload {
        if let Ok(path) = std::env::var("ASNINFO_UPLOAD_PATH") {
            info!("uploading {} to {} ...", &cli.path, path);
            if oneio::s3_env_check().is_err() {
                error!("S3 environment variables not set, skipping upload");
            } else {
                let (bucket, key) = oneio::s3_url_parse(&path).unwrap();
                oneio::s3_upload(&bucket, &key, &cli.path).unwrap();
            }
        }
    }
}
