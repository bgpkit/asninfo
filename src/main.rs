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

use axum::{extract::Query, routing::get, Json, Router};
use bgpkit_commons::asinfo::AsInfo;
use chrono::{SecondsFormat, Utc};
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::net::SocketAddr;
use std::process::exit;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio::time::sleep;
use tracing::{error, info};

#[derive(Parser)]
#[clap(author, version, about, long_about = None)]
#[clap(propagate_version = true)]
struct Cli {
    #[clap(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug, Clone)]
enum Commands {
    /// Generate ASN info dump file (JSON/JSONL/CSV) and optionally upload
    Generate {
        /// Export data path; determines format by extension (json, jsonl, csv)
        #[clap(default_value = "./asninfo.jsonl")]
        path: String,
        /// Simplified format (also implied when CSV)
        #[clap(short, long)]
        simplified: bool,
    },
    /// Serve an HTTP API for ASN info lookup
    Serve {
        /// Bind address, e.g., 0.0.0.0:8080
        #[clap(short, long, default_value = "0.0.0.0:8080")]
        bind: String,
        /// Refresh interval in seconds for background updates
        #[clap(long, default_value_t = 21600)]
        refresh_secs: u64,
        /// Use simplified mode (skip heavy datasets); default false
        #[clap(long, default_value_t = false)]
        simplified: bool,
    },
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

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AsInfoOut {
    #[serde(flatten)]
    inner: AsInfo,
    #[serde(rename = "country_name")]
    country_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LookupResponse<T> {
    data: Vec<T>,
    count: usize,
    #[serde(rename = "updatedAt")]
    updated_at: String,
    page: usize,
    page_size: usize,
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

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().with_ansi(false).init();
    dotenvy::dotenv().ok();

    let cli = Cli::parse();

    match cli.command {
        Commands::Generate { path, simplified } => {
            if let Err(code) = generate_cmd(&path, simplified) {
                exit(code);
            }
        }
        Commands::Serve {
            bind,
            refresh_secs,
            simplified,
        } => {
            if let Err(code) = serve_cmd(&bind, refresh_secs, simplified).await {
                exit(code);
            }
        }
    }
}

fn generate_cmd(path: &str, simplified_flag: bool) -> Result<(), i32> {
    let format: ExportFormat = if path.contains(".jsonl") {
        ExportFormat::JSONL
    } else if path.contains(".csv") {
        ExportFormat::CSV
    } else if path.contains(".json") {
        ExportFormat::JSON
    } else {
        error!("unknown format. please choose from csv, json, jsonl format");
        return Err(1);
    };

    let simplified = simplified_flag || matches!(format, ExportFormat::CSV);

    let load_population = !simplified;
    let load_hegemony = !simplified;
    let load_peeringdb = !simplified;

    info!("loading asn info data ...");
    let mut commons = bgpkit_commons::BgpkitCommons::new();
    if let Err(e) = commons.load_asinfo(true, load_population, load_hegemony, load_peeringdb) {
        error!("failed to load asn info data: {e}");
        return Err(1);
    };
    if let Err(e) = commons.load_countries() {
        error!("failed to load countries: {e}");
        return Err(2);
    };
    let as_info_map = commons.asinfo_all().expect("failed to get asinfo map");

    info!("export format: {}", &format);

    info!("writing asn info data to '{}' ...", &path);
    let mut writer = oneio::get_writer(&path).unwrap();
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
        info!("uploading {} to {} ...", &path, upload_path);
        if oneio::s3_env_check().is_err() {
            error!("S3 environment variables not set, skipping upload");
            return Err(3);
        } else {
            let (bucket, key) = oneio::s3_url_parse(&upload_path).unwrap();
            match oneio::s3_upload(&bucket, &key, &path) {
                Ok(_) => {
                    // try to do send a success message to
                    if let Ok(heartbeat_url) = dotenvy::var("ASNINFO_HEARTBEAT_URL") {
                        info!("sending heartbeat to configured URL");
                        if let Err(e) = oneio::read_to_string(&heartbeat_url) {
                            error!("failed to send heartbeat: {e}");
                            return Err(4);
                        }
                    }
                }
                Err(e) => {
                    error!("failed to upload to destination ({upload_path}): {e}");
                    return Err(5);
                }
            }
        }
    }
    info!("asninfo download done");
    Ok(())
}

// ==================== Serve command implementation ====================
#[derive(Clone)]
struct AppState {
    map: Arc<Mutex<HashMap<u32, AsInfoOut>>>,
    updated_at: Arc<Mutex<String>>,
}

#[derive(Deserialize)]
struct LookupQuery {
    asns: Option<String>,
    legacy: Option<bool>,
}

#[derive(Deserialize)]
struct LookupBody {
    asns: Vec<u32>,
}

async fn serve_cmd(bind: &str, refresh_secs: u64, simplified: bool) -> Result<(), i32> {
    let (initial_map, updated_at_str) = load_asn_map_out(simplified)?;
    let map = Arc::new(Mutex::new(initial_map));
    let updated_at = Arc::new(Mutex::new(updated_at_str));
    let state = AppState {
        map: map.clone(),
        updated_at: updated_at.clone(),
    };

    // start background updater
    let _handle = start_updater(map.clone(), updated_at.clone(), refresh_secs, simplified);

    // set up routes
    let app = Router::new()
        .route("/lookup", get(get_lookup).post(post_lookup))
        .with_state(state);

    let addr: SocketAddr = bind.parse().map_err(|e| {
        error!("invalid bind address {bind}: {e}");
        6
    })?;
    let listener = tokio::net::TcpListener::bind(addr).await.map_err(|e| {
        error!("failed to bind {bind}: {e}");
        6
    })?;
    info!("serving on http://{}", addr);
    axum::serve(listener, app).await.map_err(|e| {
        error!("server error: {e}");
        7
    })?;

    Ok(())
}

fn load_asn_map_out(simplified: bool) -> Result<(HashMap<u32, AsInfoOut>, String), i32> {
    let load_population = !simplified;
    let load_hegemony = !simplified;
    let load_peeringdb = !simplified;

    info!("loading asn info data ...");
    let mut commons = bgpkit_commons::BgpkitCommons::new();
    if let Err(e) = commons.load_asinfo(true, load_population, load_hegemony, load_peeringdb) {
        error!("failed to load asn info data: {e}");
        return Err(1);
    };
    if let Err(e) = commons.load_countries() {
        error!("failed to load countries: {e}");
        return Err(2);
    };
    let as_info_map = commons.asinfo_all().expect("failed to get asinfo map");

    // build enriched map with country_name
    let mut out: HashMap<u32, AsInfoOut> = HashMap::with_capacity(as_info_map.len());
    for (asn, info) in as_info_map.iter() {
        let country_name = commons
            .country_by_code(&info.country)
            .ok()
            .flatten()
            .map(|c| c.name)
            .unwrap_or_default();
        out.insert(
            *asn,
            AsInfoOut {
                inner: info.clone(),
                country_name,
            },
        );
    }
    let updated_at = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);

    Ok((out, updated_at))
}

fn start_updater(
    map: Arc<Mutex<HashMap<u32, AsInfoOut>>>,
    updated_at: Arc<Mutex<String>>,
    refresh_secs: u64,
    simplified: bool,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let interval = Duration::from_secs(refresh_secs.max(60)); // minimum 60s
        loop {
            sleep(interval).await;
            info!("background updater: refreshing ASN data ...");
            match load_asn_map_out(simplified) {
                Ok((new_map, ts)) => {
                    {
                        let mut guard = map.lock().unwrap();
                        *guard = new_map;
                    }
                    {
                        let mut ts_guard = updated_at.lock().unwrap();
                        *ts_guard = ts;
                    }
                    info!("background updater: ASN data updated");
                }
                Err(e) => {
                    error!("background updater: refresh failed with code {e}");
                }
            }
        }
    })
}

use axum::extract::State;

async fn get_lookup(State(state): State<AppState>, Query(q): Query<LookupQuery>) -> Json<Value> {
    let asns: Vec<u32> = q
        .asns
        .clone()
        .unwrap_or_default()
        .split(',')
        .filter_map(|s| u32::from_str(s.trim()).ok())
        .collect();
    let legacy = q.legacy.unwrap_or(false);
    let data_full = lookup(&state, &asns);
    let updated_at = state.updated_at.lock().unwrap().clone();

    let data_values: Vec<Value> = if legacy {
        data_full
            .into_iter()
            .map(|o| {
                let (org_id, org_name) = match &o.inner.as2org {
                    None => ("".to_string(), "".to_string()),
                    Some(v) => (v.org_id.clone(), v.org_name.clone()),
                };
                json!({
                    "asn": o.inner.asn,
                    "as_name": o.inner.name,
                    "org_id": org_id,
                    "org_name": org_name,
                    "country_code": o.inner.country,
                    "country_name": o.country_name,
                    "data_source": "",
                })
            })
            .collect()
    } else {
        data_full.into_iter().map(|o| json!(o)).collect()
    };

    let resp = json!({
        "data": data_values,
        "count": data_values.len(),
        "updatedAt": updated_at,
        "page": 0,
        "page_size": asns.len(),
    });
    Json(resp)
}

async fn post_lookup(
    State(state): State<AppState>,
    Query(q): Query<LookupQuery>,
    Json(body): Json<LookupBody>,
) -> Json<Value> {
    let legacy = q.legacy.unwrap_or(false);
    let asns = body.asns;
    let data_full = lookup(&state, &asns);
    let updated_at = state.updated_at.lock().unwrap().clone();

    let data_values: Vec<Value> = if legacy {
        data_full
            .into_iter()
            .map(|o| {
                let (org_id, org_name) = match &o.inner.as2org {
                    None => ("".to_string(), "".to_string()),
                    Some(v) => (v.org_id.clone(), v.org_name.clone()),
                };
                json!({
                    "asn": o.inner.asn,
                    "as_name": o.inner.name,
                    "org_id": org_id,
                    "org_name": org_name,
                    "country_code": o.inner.country,
                    "country_name": o.country_name,
                    "data_source": "",
                })
            })
            .collect()
    } else {
        data_full.into_iter().map(|o| json!(o)).collect()
    };

    let resp = json!({
        "data": data_values,
        "count": data_values.len(),
        "updatedAt": updated_at,
        "page": 0,
        "page_size": asns.len(),
    });
    Json(resp)
}

fn lookup(state: &AppState, asns: &[u32]) -> Vec<AsInfoOut> {
    let map = state.map.lock().unwrap();
    let mut res = Vec::with_capacity(asns.len());
    for asn in asns {
        if let Some(info) = map.get(asn) {
            res.push(info.clone());
        }
    }
    res
}
