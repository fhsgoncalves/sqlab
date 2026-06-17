use chrono::{DateTime, SecondsFormat, Utc};

pub const APP_NAME: &str = "sqlab";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const COMMIT_HASH: &str = match option_env!("SQLAB_COMMIT_HASH") {
    Some(hash) => hash,
    None => "unknown",
};
const BUILD_UNIX_TIMESTAMP: &str = match option_env!("SQLAB_BUILD_UNIX_TIMESTAMP") {
    Some(timestamp) => timestamp,
    None => "unknown",
};

pub fn build_time() -> String {
    BUILD_UNIX_TIMESTAMP
        .parse::<i64>()
        .ok()
        .and_then(|timestamp| DateTime::<Utc>::from_timestamp(timestamp, 0))
        .map(|datetime| datetime.to_rfc3339_opts(SecondsFormat::Secs, true))
        .unwrap_or_else(|| "unknown".to_string())
}

pub fn copy_text() -> String {
    format!(
        "{APP_NAME} {VERSION}\nBuild time: {}\nCommit: {COMMIT_HASH}",
        build_time()
    )
}
