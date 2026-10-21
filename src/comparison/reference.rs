use crate::{
    AppState,
    error::{ApiError, ApiResult},
    upstream::{cache, sources::Reader},
};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Clone, Deserialize, Serialize, FromRow)]
pub(super) struct Reference {
    pub storage_key: String,
    pub sha512: String,
    pub size: i64,
}
pub(super) async fn resolve(
    state: &AppState,
    reader: &Reader,
    id: &str,
    version: &str,
) -> ApiResult<Reference> {
    crate::feeds::check_package(state, id)?;
    if !id.starts_with(&reader.prefix) {
        return Err(ApiError::forbidden("Package is outside this key's prefix"));
    }
    if cache::local_exists(state, id).await? {
        return sqlx::query_as("SELECT storage_key,sha512,size FROM package_versions WHERE package_id=$1 AND version=$2").bind(id).bind(version).fetch_optional(&state.db).await?.ok_or_else(ApiError::not_found);
    }
    cache::pinned(state, reader, id).await?;
    sqlx::query_as(
        "SELECT storage_key,sha512,size FROM upstream_cache WHERE package_id=$1 AND version=$2",
    )
    .bind(id)
    .bind(version)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(ApiError::not_found)
}
pub(super) async fn authorize(
    state: &AppState,
    reader: &Reader,
    id: &str,
    baseline: &str,
    target: &str,
    inputs: &[Reference; 2],
) -> ApiResult<()> {
    for (version, expected) in [baseline, target].into_iter().zip(inputs) {
        let current = resolve(state, reader, id, version).await?;
        if current.storage_key != expected.storage_key || current.sha512 != expected.sha512 {
            return Err(ApiError::not_found());
        }
    }
    Ok(())
}
