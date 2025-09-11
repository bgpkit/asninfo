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
use std::fmt::{Display, Formatter};
use std::process::exit;
use tracing::{error, info};

#[derive(Parser)]
#[clap(author, version, about, long_about = None)]
#[clap(propagate_version = true)]
struct Cli {
    /// Export data path
    #[clap(default_value = "./asninfo.jsonl")]
    path: String,

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

#[derive(Clone, Copy)]
#[allow(clippy::upper_case_acronyms)]
enum ExportFormat {
    JSON,
    JSONL,
    CSV,
}

impl Display for ExportFormat {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ExportFormat::JSON => {
                write!(f, "json")
            }
            ExportFormat::JSONL => {
                write!(f, "jsonl")
            }
            ExportFormat::CSV => {
                write!(f, "csv")
            }
        }
    }
}

fn main() {
    tracing_subscriber::fmt().with_ansi(false).init();
    let cli = Cli::parse();

    dotenvy::dotenv().ok();

    let format: ExportFormat = if cli.path.contains(".jsonl") {
        ExportFormat::JSONL
    } else if cli.path.contains(".csv") {
        ExportFormat::CSV
    } else if cli.path.contains(".json") {
        ExportFormat::JSON
    } else {
        error!("unknown format. please choose from csv, json, jsonl format");
        exit(1);
    };

    let simplified = cli.simplified || matches!(format, ExportFormat::CSV);

    let load_population = !simplified;
    let load_hegemony = !simplified;
    let load_peeringdb = !simplified;

    info!("loading asn info data ...");
    let mut commons = bgpkit_commons::BgpkitCommons::new();
    if let Err(e) = commons.load_asinfo(true, load_population, load_hegemony, load_peeringdb) {
        error!("failed to load asn info data: {e}");
        exit(1);
    };
    if let Err(e) = commons.load_countries() {
        error!("failed to load countries: {e}");
        exit(2);
    };
    let as_info_map = commons.asinfo_all().expect("failed to get asinfo map");

    info!("export format: {}", &format);

    info!("writing asn info data to '{}' ...", &cli.path);
    let mut writer = oneio::get_writer(&cli.path).unwrap();
    let mut info_vec = as_info_map.values().collect::<Vec<_>>();
    info_vec.sort_by(|a, b| a.asn.cmp(&b.asn));

    match format {
        ExportFormat::JSON | ExportFormat::JSONL => {
            let values_vec: Vec<Value> = match simplified {
                false => info_vec.into_iter().map(|v| json!(v)).collect(),
                true => info_vec
                    .into_iter()
                    .map(|v| {
                        let mut info = AsInfoSimplified::from(v);
                        if let Ok(Some(name)) = commons.country_by_code(&info.country_code) {
                            info.country_name = name.name
                        }
                        json!(info)
                    })
                    .collect(),
            };
            if matches!(format, ExportFormat::JSONL) {
                for as_info in values_vec {
                    writeln!(writer, "{}", serde_json::to_string(&as_info).unwrap()).unwrap();
                }
            } else {
                writeln!(writer, "{}", serde_json::to_string(&values_vec).unwrap()).unwrap();
            }
        }
        ExportFormat::CSV => {
            writeln!(
                writer,
                "asn,as_name,org_id,org_name,country_code,country_name,data_source"
            )
            .unwrap();
            for asninfo in info_vec {
                let mut info = AsInfoSimplified::from(asninfo);
                if let Ok(Some(name)) = commons.country_by_code(&info.country_code) {
                    info.country_name = name.name
                }
                writeln!(
                    writer,
                    r#"{},"{}","{}","{}","{}","{}","""#,
                    info.asn,
                    info.as_name.replace('"', ""),
                    info.org_id,
                    info.org_name.replace('"', ""),
                    info.country_code,
                    info.country_name
                )
                .unwrap();
            }
        }
    }
    drop(writer);

    if let Ok(upload_path) = std::env::var("ASNINFO_UPLOAD_PATH") {
        info!("uploading {} to {} ...", &cli.path, upload_path);
        if oneio::s3_env_check().is_err() {
            error!("S3 environment variables not set, skipping upload");
            exit(3);
        } else {
            let (bucket, key) = oneio::s3_url_parse(&upload_path).unwrap();
            match oneio::s3_upload(&bucket, &key, &cli.path) {
                Ok(_) => {
                    // try to do send a success message to
                    if let Ok(heartbeat_url) = dotenvy::var("ASNINFO_HEARTBEAT_URL") {
                        info!("sending heartbeat to configured URL");
                        if let Err(e) = oneio::read_to_string(&heartbeat_url) {
                            error!("failed to send heartbeat: {e}");
                            exit(4);
                        }
                    }
                }
                Err(e) => {
                    error!("failed to upload to destination ({upload_path}): {e}");
                    exit(5);
                }
            }
        }
    }
    info!("asninfo download done");
}
