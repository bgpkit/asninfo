use axum::{
    extract::{Query, Request as AxumRequest, State},
    http::{Method, StatusCode},
    middleware::{self, Next},
    response::Response,
    routing::get,
    Json, Router,
};
use bgpkit_commons::asinfo::AsInfo;
use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio::time::sleep;
use tower_http::cors::{Any, CorsLayer};
use tracing::{error, info};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AsInfoOut {
    #[serde(flatten)]
    pub inner: AsInfo,
    #[serde(rename = "country_name")]
    pub country_name: String,
}

#[derive(Clone)]
pub struct AppState {
    pub map: Arc<Mutex<HashMap<u32, AsInfoOut>>>,
    pub updated_at: Arc<Mutex<String>>,
    pub max_asns: usize,
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

pub fn build_router(state: AppState) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST])
        .allow_headers(Any);

    Router::new()
        .route("/lookup", get(get_lookup).post(post_lookup))
        .route("/health", get(health))
        .with_state(state)
        // log all requests except /health
        .layer(middleware::from_fn(log_requests))
        .layer(cors)
}

// Middleware to log requests, skipping /health
async fn log_requests(req: AxumRequest, next: Next) -> Response {
    let path = req.uri().path().to_string();
    if path == "/health" {
        return next.run(req).await;
    }
    let method = req.method().clone();
    let start = std::time::Instant::now();
    let response = next.run(req).await;
    let status = response.status();
    let elapsed_ms = start.elapsed().as_millis();
    info!(
        method = %method,
        path = %path,
        status = %status.as_u16(),
        latency_ms = elapsed_ms,
        "request"
    );
    response
}

pub fn load_asn_map_out(simplified: bool) -> Result<(HashMap<u32, AsInfoOut>, String), i32> {
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
    let as_info_map = match commons.asinfo_all() {
        Ok(map) => map,
        Err(e) => {
            error!("failed to get asinfo map: {e}");
            return Err(3);
        }
    };

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

const MINIMUM_UPDATER_INTERVAL_SECS: u64 = 3600;

pub fn start_updater(
    map: Arc<Mutex<HashMap<u32, AsInfoOut>>>,
    updated_at: Arc<Mutex<String>>,
    refresh_secs: u64,
    simplified: bool,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let interval = Duration::from_secs(refresh_secs.max(MINIMUM_UPDATER_INTERVAL_SECS)); // minimum 1 hour
        loop {
            sleep(interval).await;
            info!("background updater: refreshing ASN data ...");
            match load_asn_map_out(simplified) {
                Ok((new_map, ts)) => {
                    // Update both map and updated_at within a single critical section
                    // to avoid exposing an inconsistent state between them.
                    let mut map_guard = map.lock().unwrap_or_else(|poisoned| {
                        error!("background updater: map mutex is poisoned, recovering");
                        poisoned.into_inner()
                    });
                    let mut ts_guard = updated_at.lock().unwrap_or_else(|poisoned| {
                        error!("background updater: updated_at mutex is poisoned, recovering");
                        poisoned.into_inner()
                    });
                    *map_guard = new_map;
                    *ts_guard = ts;
                    info!("background updater: ASN data updated");
                }
                Err(e) => {
                    error!("background updater: refresh failed with code {e}");
                }
            }
        }
    })
}

async fn health(State(state): State<AppState>) -> Json<Value> {
    let updated_at = state
        .updated_at
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    Json(json!({
        "status": "ok",
        "updatedAt": updated_at,
    }))
}

fn convert_to_legacy(list: Vec<AsInfoOut>) -> Vec<Value> {
    let mut out = Vec::with_capacity(list.len());
    for o in list.into_iter() {
        let asn = o.inner.asn;
        let as_name = o.inner.name.clone();
        let country_code = o.inner.country.clone();
        let country_name = o.country_name.clone();
        let org_id = o
            .inner
            .as2org
            .as_ref()
            .map(|v| v.org_id.clone())
            .unwrap_or_default();
        let org_name = o
            .inner
            .as2org
            .as_ref()
            .map(|v| v.org_name.clone())
            .unwrap_or_default();
        out.push(json!({
            "asn": asn,
            "as_name": as_name,
            "org_id": org_id,
            "org_name": org_name,
            "country_code": country_code,
            "country_name": country_name,
            "data_source": "",
        }));
    }
    out
}

async fn get_lookup(
    State(state): State<AppState>,
    Query(q): Query<LookupQuery>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let asns: Vec<u32> = q
        .asns
        .clone()
        .unwrap_or_default()
        .split(',')
        .filter_map(|s| s.trim().parse::<u32>().ok())
        .collect();

    if asns.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "no valid ASNs provided in 'asns' query parameter"})),
        ));
    }

    if asns.len() > state.max_asns {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(
                json!({"error": format!("payload too large, max ASNs per request is {}", state.max_asns)}),
            ),
        ));
    }

    let map_guard = state.map.lock().map_err(|_| {
        error!("get_lookup: map mutex is poisoned");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "internal server error"})),
        )
    })?;

    let mut found = Vec::with_capacity(asns.len());
    for asn in asns {
        if let Some(info) = map_guard.get(&asn) {
            found.push(info.clone());
        }
    }

    let use_legacy = q.legacy.unwrap_or(false);
    let results = if use_legacy {
        json!(convert_to_legacy(found))
    } else {
        json!(found)
    };

    Ok(Json(results))
}

async fn post_lookup(
    State(state): State<AppState>,
    Json(body): Json<LookupBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if body.asns.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "no ASNs provided in request body"})),
        ));
    }
    if body.asns.len() > state.max_asns {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(
                json!({"error": format!("payload too large, max ASNs per request is {}", state.max_asns)}),
            ),
        ));
    }

    let map_guard = state.map.lock().map_err(|_| {
        error!("post_lookup: map mutex is poisoned");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": "internal server error"})),
        )
    })?;

    let mut found = Vec::with_capacity(body.asns.len());
    for asn in body.asns {
        if let Some(info) = map_guard.get(&asn) {
            found.push(info.clone());
        }
    }

    Ok(Json(json!(found)))
}
