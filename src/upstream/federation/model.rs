use crate::{
    error::{ApiError, ApiResult},
    registry::SearchQuery,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Query {
    #[serde(default)]
    pub q: String,
    #[serde(default)]
    pub skip: i64,
    pub take: Option<i64>,
    #[serde(default)]
    pub prerelease: bool,
    pub sem_ver_level: Option<String>,
    pub source_id: Option<Uuid>,
}
impl Query {
    pub fn validate(&self) -> ApiResult<()> {
        if self.q.len() > 256
            || self.skip < 0
            || self.skip > 1000000
            || self.take.is_some_and(|n| !(1..=100).contains(&n))
        {
            return Err(ApiError::bad(
                "Federated search allows q up to 256 characters, skip 0–1000000 and take 1–100",
            ));
        }
        Ok(())
    }
    pub fn local(&self) -> SearchQuery {
        SearchQuery {
            q: self.q.clone(),
            skip: self.skip.min(100000),
            take: self.take,
            prerelease: self.prerelease,
            sem_ver_level: self.sem_ver_level.clone(),
            ..Default::default()
        }
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourcePage {
    pub source_id: Uuid,
    pub name: String,
    pub upstream_total_hits: Option<u64>,
    pub skip: i64,
    pub next_skip: Option<i64>,
    pub data: Vec<Value>,
    pub error: Option<String>,
}
