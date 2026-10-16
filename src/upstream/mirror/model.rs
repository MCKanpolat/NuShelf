use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::FromRow;
use uuid::Uuid;
#[derive(FromRow)]
pub(super) struct Mirror {
    pub enabled: bool,
    pub requested: bool,
    pub status: String,
    pub generation: Option<Uuid>,
    pub source_url: Option<String>,
    pub cursor: Value,
    pub discovery_complete: bool,
}
#[derive(FromRow)]
pub(super) struct Job {
    pub package_id: String,
    pub version: String,
    pub action: String,
    pub event_time: DateTime<Utc>,
    pub attempts: i32,
}
#[derive(Clone)]
pub(super) struct Discovered {
    pub id: String,
    pub version: String,
    pub action: &'static str,
    pub at: DateTime<Utc>,
}
pub(super) struct Discovery {
    pub jobs: Vec<Discovered>,
    pub cursor: Value,
    pub complete: bool,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Settings {
    pub enabled: bool,
    pub interval_seconds: i32,
}
