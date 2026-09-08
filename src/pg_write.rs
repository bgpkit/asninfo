//! PostgreSQL bulk-write support for the BGPKIT ASN-info dataset
//! (`asninfo pg-write`).
//!
//! The loader performs a full refresh with zero lookups: the entire dataset is
//! streamed into a staging table via CSV-format COPY, indexed, and swapped over
//! the live `asninfo.current` table in a single transaction, so API readers
//! never observe an empty table. Every run (success or failure) is recorded in
//! `asninfo.ingest_run` for provenance.
//!
//! Row shape of `asninfo.current`: typed columns for the search surface
//! (`asn`, `name`, `country`, `country_name`, `org_id`, `org_name`), one JSONB
//! column per remaining source (`population`, `hegemony`, `peeringdb`; SQL NULL
//! when a source carries no data for the ASN), and provenance columns
//! (`data_as_of`, `source_revision`).
//!
//! Required privileges: the connecting role needs `USAGE` + `CREATE` on the
//! `asninfo` schema, or database-level `CREATE` so the loader can create the
//! schema on the first run. The loader owns the tables it creates, which is
//! what the staging swap requires.

use bgpkit_commons::asinfo::AsInfo;
use bgpkit_commons::BgpkitCommons;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use futures_util::SinkExt;
use serde::Serialize;
use std::pin::pin;
use tokio_postgres::NoTls;
use tracing::{error, info, warn};

/// Build revision recorded in `source_revision` and `ingest_run`.
const SOURCE_REVISION: &str = env!("CARGO_PKG_VERSION");

/// COPY payload chunk size (bytes) flushed to PostgreSQL at a time.
const COPY_CHUNK_BYTES: usize = 512 * 1024;

const STAGING_TABLE: &str = "asninfo.current_staging";

/// Provenance table (in the style of `meta.ingest_run`, but kept in the
/// `asninfo` schema so the loader role only ever needs privileges there).
const INGEST_RUN_DDL: &str = "CREATE TABLE IF NOT EXISTS asninfo.ingest_run (
    run_id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    task text NOT NULL,
    status text NOT NULL,
    row_count bigint,
    data_as_of timestamptz,
    source_revision text NOT NULL,
    started_at timestamptz NOT NULL,
    finished_at timestamptz NOT NULL,
    duration_secs double precision NOT NULL,
    error text
)";

/// Explicit staging DDL: `asninfo.current` is only ever created by renaming a
/// fully-loaded, indexed staging table, so it never exists as an empty table.
const STAGING_DDL: &str = "CREATE TABLE asninfo.current_staging (
    asn bigint NOT NULL,
    name text NOT NULL,
    country text NOT NULL,
    country_name text NOT NULL,
    org_id text,
    org_name text,
    population jsonb,
    hegemony jsonb,
    peeringdb jsonb,
    data_as_of timestamptz NOT NULL,
    source_revision text NOT NULL
)";

const COPY_SQL: &str = "COPY asninfo.current_staging (asn, name, country, country_name, org_id, org_name, population, hegemony, peeringdb, data_as_of, source_revision) FROM STDIN WITH (FORMAT csv)";

const INGEST_INSERT_SQL: &str = "INSERT INTO asninfo.ingest_run (task, status, row_count, data_as_of, source_revision, started_at, finished_at, duration_secs, error) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)";

const PG_TRGM_SCHEMA_SQL: &str = "SELECT n.nspname FROM pg_extension e JOIN pg_namespace n ON e.extnamespace = n.oid WHERE e.extname = 'pg_trgm'";

/// Session-level advisory lock key (0x41534E494E464F21, ASCII "ASNINFO!") held
/// for the whole load: concurrent `pg-write` runs share the staging table and
/// would interfere, so they serialize on this lock. Released automatically
/// when the connection closes.
const ADVISORY_LOCK_KEY: i64 = 0x4153_4E49_4E46_4F21;

/// Bulk-write the full ASN info dataset into PostgreSQL.
///
/// On failure the error is logged, a best-effort `ingest_run` row with status
/// `error` is recorded, and the mapped exit code is returned.
pub async fn pg_write_cmd(database_url: &str) -> Result<(), i32> {
    let started_at = Utc::now();
    match execute_pg_write(database_url, started_at).await {
        Ok(row_count) => {
            info!("pg-write complete: {row_count} rows in asninfo.current");
            Ok(())
        }
        Err((code, message)) => {
            error!("{message}");
            record_error_run(database_url, started_at, &message).await;
            Err(code)
        }
    }
}

async fn execute_pg_write(
    database_url: &str,
    started_at: DateTime<Utc>,
) -> Result<u64, (i32, String)> {
    // Full dataset build, same source path as `generate`: ASN names, CAIDA
    // as2org, APNIC population, IIJ hegemony, PeeringDB profiles, countries.
    // The load is synchronous and internally creates/drops blocking HTTP
    // clients that own their own tokio runtimes; dropping such a runtime
    // inside an async context panics (tokio >= 1.48), so it must run on the
    // blocking pool instead.
    let (commons, infos) = tokio::task::spawn_blocking(load_dataset)
        .await
        .map_err(|e| (11, format!("data loader task failed: {e}")))??;
    let data_as_of = Utc::now();

    info!("connecting to PostgreSQL ...");
    let (mut client, connection) = match tokio_postgres::connect(database_url, NoTls).await {
        Ok(pair) => pair,
        Err(e) => return Err((14, format!("failed to connect to PostgreSQL: {e}"))),
    };
    tokio::spawn(async move {
        if let Err(e) = connection.await {
            error!("postgres connection error: {e}");
        }
    });
    // Serialize concurrent runs before any DDL/COPY work happens.
    client
        .execute("SELECT pg_advisory_lock($1)", &[&ADVISORY_LOCK_KEY])
        .await
        .map_err(|e| (15, format!("failed to acquire advisory lock: {e}")))?;

    run_load(&mut client, &commons, &infos, data_as_of, started_at).await
}

/// Load the full ASN-info dataset from `bgpkit-commons`, returning the commons
/// handle (for country lookups) and the ASN-sorted info rows.
fn load_dataset() -> Result<(BgpkitCommons, Vec<AsInfo>), (i32, String)> {
    info!("loading asn info data ...");
    let mut commons = BgpkitCommons::new();
    if let Err(e) = commons.load_asinfo(true, true, true, true) {
        return Err((11, format!("failed to load asn info data: {e}")));
    }
    if let Err(e) = commons.load_countries() {
        return Err((12, format!("failed to load countries: {e}")));
    }
    let as_info_map = match commons.asinfo_all() {
        Ok(map) => map,
        Err(e) => return Err((13, format!("failed to get asinfo map: {e}"))),
    };
    let mut infos: Vec<AsInfo> = as_info_map.into_values().collect();
    infos.sort_by_key(|i| i.asn);
    Ok((commons, infos))
}

async fn run_load(
    client: &mut tokio_postgres::Client,
    commons: &BgpkitCommons,
    infos: &[AsInfo],
    data_as_of: DateTime<Utc>,
    started_at: DateTime<Utc>,
) -> Result<u64, (i32, String)> {
    ensure_schema(client).await?;
    client
        .batch_execute(INGEST_RUN_DDL)
        .await
        .map_err(|e| (15, format!("failed to create asninfo.ingest_run: {e}")))?;
    client
        .batch_execute(&format!("DROP TABLE IF EXISTS {STAGING_TABLE}"))
        .await
        .map_err(|e| (15, format!("failed to drop staging table: {e}")))?;
    client
        .batch_execute(STAGING_DDL)
        .await
        .map_err(|e| (15, format!("failed to create staging table: {e}")))?;

    // Streaming COPY: rows are mapped to CSV lines and fed to PostgreSQL in
    // bounded chunks.
    let sink = client
        .copy_in(COPY_SQL)
        .await
        .map_err(|e| (15, format!("failed to start COPY: {e}")))?;
    let mut sink = pin!(sink);
    let data_as_of_str = data_as_of.to_rfc3339();
    let mut buf: Vec<u8> = Vec::with_capacity(COPY_CHUNK_BYTES);
    for info in infos {
        let country_name = commons
            .country_by_code(&info.country)
            .ok()
            .flatten()
            .map(|c| c.name)
            .unwrap_or_default();
        let line = map_as_info_row(info, &country_name, &data_as_of_str)
            .map_err(|e| (16, format!("failed to serialize AS{}: {e}", info.asn)))?;
        buf.extend_from_slice(line.as_bytes());
        buf.push(b'\n');
        if buf.len() >= COPY_CHUNK_BYTES {
            sink.as_mut()
                .feed(Bytes::from(std::mem::take(&mut buf)))
                .await
                .map_err(|e| (15, format!("failed to stream COPY data: {e}")))?;
        }
    }
    if !buf.is_empty() {
        sink.as_mut()
            .feed(Bytes::from(buf))
            .await
            .map_err(|e| (15, format!("failed to stream COPY data: {e}")))?;
    }
    let copied_rows = sink
        .finish()
        .await
        .map_err(|e| (15, format!("failed to finish COPY: {e}")))?;
    info!("COPY complete: {copied_rows} rows in staging table");

    // Guard against replacing a healthy table with a broken snapshot: an
    // empty load, or one with fewer than half the currently loaded rows, is
    // almost certainly a source failure, not a legitimate refresh.
    let existing_rows = if table_exists(client, "asninfo.current").await? {
        let n: i64 = client
            .query_one("SELECT count(*) FROM asninfo.current", &[])
            .await
            .map_err(|e| (15, format!("failed to count asninfo.current rows: {e}")))?
            .get(0);
        Some(n)
    } else {
        None
    };
    check_swap_safety(existing_rows, copied_rows)
        .map_err(|message| (15, format!("{message} (staging table left for inspection)")))?;

    // Integrity indexes: failure here is a hard error (the leftover staging
    // table is dropped by the next run).
    client
        .batch_execute(
            "ALTER TABLE asninfo.current_staging ADD CONSTRAINT current_staging_pkey PRIMARY KEY (asn)",
        )
        .await
        .map_err(|e| (15, format!("failed to add primary key on staging: {e}")))?;
    client
        .batch_execute(
            "CREATE INDEX current_staging_country_idx ON asninfo.current_staging (country)",
        )
        .await
        .map_err(|e| {
            (
                15,
                format!("failed to create country index on staging: {e}"),
            )
        })?;

    // Trigram GIN indexes for name/org search: only when the pg_trgm
    // extension is installed. Otherwise degrade gracefully (search falls back
    // to sequential scans) and log a warning. The operator class is qualified
    // with the extension's actual namespace so a non-public install works.
    let pg_trgm_schema = pg_trgm_schema(client).await?;
    if let Some(schema) = &pg_trgm_schema {
        for stmt in [
            format!(
                "CREATE INDEX current_staging_name_trgm_idx ON asninfo.current_staging USING gin (name {schema}.gin_trgm_ops)"
            ),
            format!(
                "CREATE INDEX current_staging_org_name_trgm_idx ON asninfo.current_staging USING gin (org_name {schema}.gin_trgm_ops)"
            ),
        ] {
            client
                .batch_execute(&stmt)
                .await
                .map_err(|e| (15, format!("failed to create trigram index on staging: {e}")))?;
        }
    } else {
        warn!("pg_trgm extension not available; skipping trigram GIN indexes on name/org_name");
    }

    // Atomic swap in a single transaction: readers always see either the old
    // or the new `asninfo.current`, never an empty table. The ingest_run row
    // commits with the swap.
    let tx = client
        .transaction()
        .await
        .map_err(|e| (15, format!("failed to begin swap transaction: {e}")))?;
    tx.batch_execute("DROP TABLE IF EXISTS asninfo.current")
        .await
        .map_err(|e| (15, format!("failed to drop asninfo.current: {e}")))?;
    tx.batch_execute("ALTER TABLE asninfo.current_staging RENAME TO current")
        .await
        .map_err(|e| (15, format!("failed to rename staging to current: {e}")))?;
    tx.batch_execute(
        "ALTER TABLE asninfo.current RENAME CONSTRAINT current_staging_pkey TO current_pkey",
    )
    .await
    .map_err(|e| (15, format!("failed to rename primary key constraint: {e}")))?;
    tx.batch_execute(
        "ALTER INDEX asninfo.current_staging_country_idx RENAME TO current_country_idx",
    )
    .await
    .map_err(|e| (15, format!("failed to rename country index: {e}")))?;
    if pg_trgm_schema.is_some() {
        tx.batch_execute(
            "ALTER INDEX asninfo.current_staging_name_trgm_idx RENAME TO current_name_trgm_idx",
        )
        .await
        .map_err(|e| (15, format!("failed to rename name trigram index: {e}")))?;
        tx.batch_execute(
            "ALTER INDEX asninfo.current_staging_org_name_trgm_idx RENAME TO current_org_name_trgm_idx",
        )
        .await
        .map_err(|e| (15, format!("failed to rename org_name trigram index: {e}")))?;
    }
    let finished_at = Utc::now();
    let duration_secs = (finished_at - started_at).num_milliseconds() as f64 / 1000.0;
    let row_count = copied_rows.min(i64::MAX as u64) as i64;
    tx.execute(
        INGEST_INSERT_SQL,
        &[
            &"pg_write",
            &"ok",
            &row_count,
            &data_as_of,
            &SOURCE_REVISION,
            &started_at,
            &finished_at,
            &duration_secs,
            &None::<&str>,
        ],
    )
    .await
    .map_err(|e| (15, format!("failed to record ingest_run: {e}")))?;
    tx.commit()
        .await
        .map_err(|e| (15, format!("failed to commit swap: {e}")))?;
    Ok(copied_rows)
}

/// Ensure the `asninfo` schema exists. A separate probe avoids relying on the
/// privilege semantics of `CREATE SCHEMA IF NOT EXISTS` for a pre-created
/// schema (which the connecting role may not have database-level CREATE for).
async fn ensure_schema(client: &tokio_postgres::Client) -> Result<(), (i32, String)> {
    let exists: bool = client
        .query_one("SELECT to_regnamespace('asninfo') IS NOT NULL", &[])
        .await
        .map_err(|e| (15, format!("failed to check for asninfo schema: {e}")))?
        .get(0);
    if !exists {
        client
            .batch_execute("CREATE SCHEMA asninfo")
            .await
            .map_err(|e| (15, format!("failed to create asninfo schema: {e}")))?;
    }
    Ok(())
}

async fn table_exists(client: &tokio_postgres::Client, name: &str) -> Result<bool, (i32, String)> {
    let exists: bool = client
        .query_one("SELECT to_regclass($1) IS NOT NULL", &[&name])
        .await
        .map_err(|e| (15, format!("failed to check for table {name}: {e}")))?
        .get(0);
    Ok(exists)
}

/// Refuse to replace a healthy table with a broken snapshot: a new dataset
/// with zero rows, or with fewer than half the currently loaded rows, is
/// treated as a source failure rather than a legitimate refresh.
fn check_swap_safety(existing_rows: Option<i64>, new_rows: u64) -> Result<(), String> {
    match existing_rows {
        Some(existing) if existing > 0 && new_rows < existing as u64 / 2 => Err(format!(
            "refusing to swap: new dataset has {new_rows} rows vs {existing} currently loaded (less than half)"
        )),
        None if new_rows == 0 => {
            Err("refusing to swap: loaded dataset is empty".to_string())
        }
        _ => Ok(()),
    }
}

/// Resolve the schema (namespace) the `pg_trgm` extension is installed into,
/// or `None` when the extension is not installed in the target database.
async fn pg_trgm_schema(client: &tokio_postgres::Client) -> Result<Option<String>, (i32, String)> {
    let row = client
        .query_opt(PG_TRGM_SCHEMA_SQL, &[])
        .await
        .map_err(|e| (15, format!("failed to probe pg_trgm extension: {e}")))?;
    Ok(row.map(|r| r.get(0)))
}

/// Best-effort provenance record for a failed run. Logs warnings only; it
/// never changes the caller's exit code. Uses a fresh connection so the
/// failure of the original client does not matter.
async fn record_error_run(database_url: &str, started_at: DateTime<Utc>, message: &str) {
    let (client, connection) = match tokio_postgres::connect(database_url, NoTls).await {
        Ok(pair) => pair,
        Err(e) => {
            warn!("could not connect to record ingest_run error: {e}");
            return;
        }
    };
    tokio::spawn(async move {
        if let Err(e) = connection.await {
            warn!("postgres connection error: {e}");
        }
    });
    // Best-effort DDL: when the failure happened before the first run's DDL
    // (e.g. the data load failed), the provenance table may not exist yet.
    let _ = client
        .batch_execute("CREATE SCHEMA IF NOT EXISTS asninfo")
        .await;
    let _ = client.batch_execute(INGEST_RUN_DDL).await;
    let finished_at = Utc::now();
    let duration_secs = (finished_at - started_at).num_milliseconds() as f64 / 1000.0;
    let record = client
        .execute(
            INGEST_INSERT_SQL,
            &[
                &"pg_write",
                &"error",
                &None::<i64>,           // row count unknown for a failed run
                &None::<DateTime<Utc>>, // no data snapshot completed
                &SOURCE_REVISION,
                &started_at,
                &finished_at,
                &duration_secs,
                &Some(message),
            ],
        )
        .await;
    if let Err(e) = record {
        warn!("failed to record ingest_run error: {e}");
    }
}

/// Map one `AsInfo` (plus the enriched `country_name`) to a CSV line for
/// `COPY ... WITH (FORMAT csv)`. `asn` is written unquoted; every other field
/// is quoted CSV. JSON columns carry the serialized source object or, when the
/// source is absent, an empty (unquoted) field which PostgreSQL reads as SQL
/// NULL.
fn map_as_info_row(
    info: &AsInfo,
    country_name: &str,
    data_as_of: &str,
) -> Result<String, serde_json::Error> {
    let (org_id, org_name) = match &info.as2org {
        Some(a2o) => (Some(a2o.org_id.clone()), Some(a2o.org_name.clone())),
        None => (None, None),
    };
    let fields: Vec<Option<String>> = vec![
        Some(info.name.clone()),
        Some(info.country.clone()),
        Some(country_name.to_string()),
        org_id,
        org_name,
        json_field(info.population.as_ref())?,
        json_field(info.hegemony.as_ref())?,
        json_field(info.peeringdb.as_ref())?,
        Some(data_as_of.to_string()),
        Some(SOURCE_REVISION.to_string()),
    ];

    let mut line = String::with_capacity(512);
    line.push_str(&info.asn.to_string());
    line.push(',');
    line.push_str(&build_csv_line(&fields));
    Ok(line)
}

/// Serialize an optional JSON column: `None` becomes SQL NULL (empty field),
/// `Some(value)` becomes the double-quoted JSON string.
fn json_field<T: Serialize>(value: Option<&T>) -> Result<Option<String>, serde_json::Error> {
    value.map(serde_json::to_string).transpose()
}

/// CSV-encode the optional field list: every `Some` value is double-quoted
/// (embedded quotes doubled); every `None` value becomes an empty unquoted
/// field, i.e. SQL NULL for PostgreSQL COPY.
fn build_csv_line(fields: &[Option<String>]) -> String {
    let mut line = String::new();
    for (idx, field) in fields.iter().enumerate() {
        if idx > 0 {
            line.push(',');
        }
        if let Some(value) = field {
            push_csv_field(&mut line, value)
        }
    }
    line
}

/// Append `value` to `out` as a double-quoted CSV field, doubling embedded
/// double quotes so commas and newlines inside the value survive. NUL bytes
/// are dropped: they are not representable in PostgreSQL's text format.
fn push_csv_field(out: &mut String, value: &str) {
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\"\""),
            '\0' => {}
            _ => out.push(ch),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;
    use bgpkit_commons::asinfo::{As2orgInfo, AsnPopulationData};

    /// Minimal CSV parser for test assertions: handles double-quoted fields
    /// with doubled quotes and empty unquoted fields (SQL NULL).
    fn parse_csv_line(line: &str) -> Vec<Option<String>> {
        let mut out: Vec<Option<String>> = Vec::new();
        let mut cur = String::new();
        let mut in_quotes = false;
        let mut is_null = true;
        let mut chars = line.chars().peekable();
        while let Some(c) = chars.next() {
            if in_quotes {
                if c == '"' {
                    if chars.peek() == Some(&'"') {
                        chars.next();
                        cur.push('"');
                    } else {
                        in_quotes = false;
                    }
                } else {
                    cur.push(c);
                }
            } else {
                match c {
                    '"' => {
                        in_quotes = true;
                        is_null = false;
                    }
                    ',' => {
                        out.push(if is_null {
                            None
                        } else {
                            Some(std::mem::take(&mut cur))
                        });
                        is_null = true;
                    }
                    _ => {
                        cur.push(c);
                        is_null = false;
                    }
                }
            }
        }
        out.push(if is_null { None } else { Some(cur) });
        out
    }

    #[test]
    fn swap_safety_rejects_empty_first_load() {
        assert!(check_swap_safety(None, 0).is_err());
        assert!(check_swap_safety(None, 1).is_ok());
    }

    #[test]
    fn swap_safety_rejects_less_than_half_of_loaded_rows() {
        assert!(check_swap_safety(Some(100), 49).is_err());
        assert!(check_swap_safety(Some(100), 50).is_ok());
        assert!(check_swap_safety(Some(100), 101).is_ok());
        assert!(check_swap_safety(Some(122_372), 122_371).is_ok());
    }

    #[test]
    fn csv_field_quotes_doubled() {
        let mut out = String::new();
        push_csv_field(&mut out, "plain");
        assert_eq!(out, "\"plain\"");

        let mut out = String::new();
        push_csv_field(&mut out, "a\"b\"c");
        assert_eq!(out, "\"a\"\"b\"\"c\"");
    }

    #[test]
    fn csv_field_empty_is_quoted() {
        let mut out = String::new();
        push_csv_field(&mut out, "");
        assert_eq!(out, "\"\"");
    }

    #[test]
    fn csv_field_commas_and_newlines_survive() {
        let mut out = String::new();
        push_csv_field(&mut out, "a,b");
        assert_eq!(out, "\"a,b\"");

        let mut out = String::new();
        push_csv_field(&mut out, "line1\nline2");
        assert_eq!(out, "\"line1\nline2\"");
    }

    #[test]
    fn csv_field_nul_dropped() {
        let mut out = String::new();
        push_csv_field(&mut out, "a\0b");
        assert_eq!(out, "\"ab\"");
    }

    #[test]
    fn csv_line_primitives_and_null_as_empty_field() {
        let line = build_csv_line(&[
            Some("a,b".to_string()),
            Some(String::new()),
            None,
            Some("x\"y".to_string()),
        ]);
        assert_eq!(line, "\"a,b\",\"\",,\"x\"\"y\"");
    }

    #[test]
    fn map_row_all_fields_present() {
        let as2org = As2orgInfo {
            name: "CLOUDFLARENET".to_string(),
            country: "US".to_string(),
            org_id: "CLOUD14-ARIN".to_string(),
            org_name: "Cloudflare, Inc.".to_string(),
        };
        let population = AsnPopulationData {
            user_count: 10,
            percent_country: 0.02,
            percent_global: 0.0,
            sample_count: 127,
        };
        let info = AsInfo {
            asn: 13335,
            name: "CLOUDFLARENET".to_string(),
            country: "US".to_string(),
            as2org: Some(as2org),
            population: Some(population),
            hegemony: None,
            peeringdb: None,
        };
        let line = map_as_info_row(&info, "United States", "2026-09-08T00:00:00+00:00").unwrap();
        let fields = parse_csv_line(&line);
        assert_eq!(fields.len(), 11);
        assert_eq!(fields[0].as_deref(), Some("13335"));
        assert_eq!(fields[1].as_deref(), Some("CLOUDFLARENET"));
        assert_eq!(fields[2].as_deref(), Some("US"));
        assert_eq!(fields[3].as_deref(), Some("United States"));
        // org_id/org_name come from as2org; org_name holds an embedded comma
        assert_eq!(fields[4].as_deref(), Some("CLOUD14-ARIN"));
        assert_eq!(fields[5].as_deref(), Some("Cloudflare, Inc."));
        // JSONB columns hold the serialized source object
        let pop_json: serde_json::Value =
            serde_json::from_str(fields[6].as_deref().unwrap()).unwrap();
        assert_eq!(pop_json["user_count"], 10);
        assert_eq!(pop_json["percent_country"], 0.02);
        assert_eq!(pop_json["sample_count"], 127);
        // absent sources are SQL NULL (empty unquoted field), not JSON "null"
        assert_eq!(fields[7], None);
        assert_eq!(fields[8], None);
        assert_eq!(fields[9].as_deref(), Some("2026-09-08T00:00:00+00:00"));
        assert_eq!(fields[10].as_deref(), Some(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn map_row_no_as2org_null_fields() {
        let info = AsInfo {
            asn: 400644,
            name: "BGPKIT-LLC".to_string(),
            country: "US".to_string(),
            as2org: None,
            population: None,
            hegemony: None,
            peeringdb: None,
        };
        let line = map_as_info_row(&info, "", "2026-09-08T00:00:00+00:00").unwrap();
        let fields = parse_csv_line(&line);
        assert_eq!(fields.len(), 11);
        assert_eq!(fields[0].as_deref(), Some("400644"));
        assert_eq!(fields[1].as_deref(), Some("BGPKIT-LLC"));
        assert_eq!(fields[3].as_deref(), Some(""));
        // no as2org: org_id/org_name are SQL NULL
        assert_eq!(fields[4], None);
        assert_eq!(fields[5], None);
        assert_eq!(fields[6], None);
        assert_eq!(fields[7], None);
        assert_eq!(fields[8], None);
    }
}
