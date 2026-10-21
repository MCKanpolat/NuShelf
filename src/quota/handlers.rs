use crate::{
    AppState, auth,
    error::{ApiError, ApiResult},
};
use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
};
use serde::Deserialize;
use serde_json::{Value, json};
async fn manager(state: &AppState, headers: &HeaderMap) -> ApiResult<()> {
    if auth::is_admin(state, headers) {
        return Ok(());
    }
    let who = auth::authenticate(state, headers).await?;
    if !who.package_prefix.is_empty() {
        return Err(ApiError::forbidden(
            "Feed usage requires an unrestricted management key",
        ));
    }
    let feed = state
        .feed
        .ok_or_else(|| ApiError::forbidden("Workspace administrator required"))?;
    crate::feeds::manager(state, headers, feed).await
}
pub async fn status(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    manager(&state, &headers).await?;
    let usage:Value=sqlx::query_scalar("SELECT jsonb_build_object('packages',package_count,'versions',version_count,'packageBytes',package_bytes,'symbolBytes',symbol_bytes,'totalBytes',package_bytes+symbol_bytes,'maxBytes',max_bytes,'maxPackages',max_packages,'maxVersions',max_versions,'revision',revision,'overQuota',(max_bytes>0 AND package_bytes+symbol_bytes>max_bytes) OR (max_packages>0 AND package_count>max_packages) OR (max_versions>0 AND version_count>max_versions)) FROM registry_usage WHERE singleton").fetch_one(&state.db).await?;
    let cache:Value=sqlx::query_scalar("SELECT jsonb_build_object('versions',count(*),'bytes',COALESCE(sum(size),0)) FROM upstream_cache").fetch_one(&state.db).await?;
    let history:Vec<Value>=sqlx::query_scalar("SELECT jsonb_build_object('day',day,'packages',package_count,'versions',version_count,'packageBytes',package_bytes,'symbolBytes',symbol_bytes,'sampledAt',sampled_at) FROM registry_usage_history ORDER BY day DESC LIMIT 90").fetch_all(&state.db).await?;
    Ok(Json(
        json!({"feedId":state.feed,"usage":usage,"cache":cache,"history":history}),
    ))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Input {
    max_bytes: i64,
    max_packages: i64,
    max_versions: i64,
    revision: i64,
}
pub async fn update(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<Input>,
) -> ApiResult<StatusCode> {
    manager(&state, &headers).await?;
    // Keep API values within JavaScript's exact integer range.
    if [input.max_bytes, input.max_packages, input.max_versions]
        .iter()
        .any(|v| !(0..=9_007_199_254_740_991).contains(v))
    {
        return Err(ApiError::bad(
            "Quota values must be nonnegative safe integers; zero means unlimited",
        ));
    }
    let changed=sqlx::query("UPDATE registry_usage SET max_bytes=$1,max_packages=$2,max_versions=$3,revision=revision+1 WHERE singleton AND revision=$4")
        .bind(input.max_bytes).bind(input.max_packages).bind(input.max_versions).bind(input.revision).execute(&state.db).await?;
    if changed.rows_affected() == 0 {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "Quota changed; reload before saving".into(),
        ));
    }
    Ok(StatusCode::NO_CONTENT)
}
