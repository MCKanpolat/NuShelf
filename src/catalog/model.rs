use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::FromRow;
use uuid::Uuid;
pub(super) const PAGE_SIZE: i64 = 500;
#[derive(FromRow)]
pub(super) struct Event {
    pub event_id: Uuid,
    pub committed_at: DateTime<Utc>,
    pub package_id: String,
    pub version: String,
    pub event_type: String,
    pub payload: Value,
}
#[derive(FromRow)]
pub(super) struct PageSummary {
    pub page: i64,
    pub count: i64,
    pub event_id: Uuid,
    pub committed_at: DateTime<Utc>,
}
