use crate::{
    AppState, auth,
    error::{ApiError, ApiResult},
    version::{NuGetVersion, package_id},
};
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Input {
    pub reasons: Vec<String>,
    pub message: Option<String>,
    pub alternate_package: Option<Alternate>,
}
#[derive(Deserialize)]
pub struct Alternate {
    pub id: String,
    pub range: Option<String>,
}
pub fn value(input: Input) -> ApiResult<Value> {
    if input.reasons.is_empty()
        || input.reasons.len() > 3
        || input.message.as_ref().is_some_and(|s| s.len() > 2000)
    {
        return Err(ApiError::bad(
            "Choose 1–3 deprecation reasons and a message up to 2000 characters",
        ));
    }
    let mut reasons = Vec::new();
    for reason in input.reasons {
        let canonical = ["Legacy", "CriticalBugs", "Other"]
            .into_iter()
            .find(|r| r.eq_ignore_ascii_case(&reason))
            .ok_or_else(|| ApiError::bad("Unknown deprecation reason"))?;
        reasons.push(canonical);
    }
    reasons.sort();
    reasons.dedup();
    let mut result = json!({"reasons":reasons});
    if let Some(message) = input.message {
        result["message"] = json!(message);
    }
    if let Some(alt) = input.alternate_package {
        package_id(&alt.id)?;
        if let Some(range) = &alt.range
            && range != "*"
        {
            crate::audit::validate_range(range)?;
        }
        result["alternatePackage"] =
            json!({"id":alt.id,"range":alt.range.unwrap_or_else(||"*".into())});
    }
    Ok(result)
}
async fn set(
    state: &AppState,
    headers: &HeaderMap,
    id: &str,
    version: &str,
    data: Option<Value>,
) -> ApiResult<StatusCode> {
    let id = package_id(id)?;
    let version = NuGetVersion::parse(version)?.normalized;
    let who = if auth::is_admin(state, headers) {
        None
    } else {
        let who = auth::authenticate(state, headers).await?;
        who.require("package:deprecate")?;
        who.package(&id)?;
        Some(who)
    };
    let mut tx = state.db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(&id)
        .execute(&mut *tx)
        .await?;
    let owner: Uuid = sqlx::query_scalar("SELECT owner_id FROM packages WHERE id=$1")
        .bind(&id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if who.is_some_and(|w| w.profile_id != owner) {
        return Err(ApiError::forbidden("Package belongs to another profile"));
    }
    if sqlx::query("UPDATE package_versions SET deprecation=$3 WHERE package_id=$1 AND version=$2")
        .bind(id)
        .bind(version)
        .bind(data)
        .execute(&mut *tx)
        .await?
        .rows_affected()
        == 0
    {
        return Err(ApiError::not_found());
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
pub async fn put(
    State(state): State<AppState>,
    Path((id, version)): Path<(String, String)>,
    headers: HeaderMap,
    Json(input): Json<Input>,
) -> ApiResult<StatusCode> {
    set(&state, &headers, &id, &version, Some(value(input)?)).await
}
pub async fn clear(
    State(state): State<AppState>,
    Path((id, version)): Path<(String, String)>,
    headers: HeaderMap,
) -> ApiResult<StatusCode> {
    set(&state, &headers, &id, &version, None).await
}
