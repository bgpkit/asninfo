use tracing::{error, info};

fn main() {
    tracing_subscriber::fmt().with_ansi(false).init();

    dotenvy::dotenv().ok();

    info!("loading asn info data ...");
    let mut commons = bgpkit_commons::BgpkitCommons::new();
    commons.load_asinfo(true, true, true).unwrap();
    let as_info_map = commons.asinfo_all().unwrap();

    info!("writing asn info data to 'asninfo.jsonl' ...");
    let mut writer = oneio::get_writer("asinfo.jsonl").unwrap();
    let mut info_vec = as_info_map.values().collect::<Vec<_>>();
    info_vec.sort_by(|a, b| a.asn.cmp(&b.asn));
    for as_info in info_vec {
        writeln!(writer, "{}", serde_json::to_string(&as_info).unwrap()).unwrap();
    }

    if let Ok(path) = std::env::var("ASNINFO_UPLOAD_PATH") {
        info!("uploading asinfo.jsonl to {} ...", path);
        if oneio::s3_env_check().is_err() {
            error!("S3 environment variables not set, skipping upload");
        } else {
            let (bucket, key) = oneio::s3_url_parse(&path).unwrap();
            oneio::s3_upload(&bucket, &key, "asinfo.jsonl").unwrap();
        }
    }
    drop(writer);
}
