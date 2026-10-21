use super::reference::{self, Reference};
use crate::{
    AppState,
    error::{ApiError, ApiResult},
    upstream::sources::Reader,
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Input {
    baseline_version: String,
    target_version: String,
}
async fn reader(state: &AppState, headers: &HeaderMap) -> ApiResult<Reader> {
    let reader = Reader::from_headers(state, headers).await?;
    if !reader.authenticated {
        return Err(ApiError::unauthorized());
    }
    Ok(reader)
}
pub async fn create(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(input): Json<Input>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let id = package_id(&id)?;
    let reader = reader(&state, &headers).await?;
    let baseline = NuGetVersion::parse(&input.baseline_version)?;
    let target = NuGetVersion::parse(&input.target_version)?;
    if baseline.sort_key >= target.sort_key {
        return Err(ApiError::bad("Choose a target newer than the baseline"));
    }
    let inputs = [
        reference::resolve(&state, &reader, &id, &baseline.normalized).await?,
        reference::resolve(&state, &reader, &id, &target.normalized).await?,
    ];
    let mut tx = state.db.begin().await?;
    sqlx::query(
        "SELECT pg_advisory_xact_lock(hashtextextended(current_schema()||':comparisons',0))",
    )
    .execute(&mut *tx)
    .await?;
    let active: Option<Uuid>=sqlx::query_scalar("SELECT id FROM package_comparisons WHERE package_id=$1 AND baseline_version=$2 AND target_version=$3 AND status IN ('queued','running')").bind(&id).bind(&baseline.normalized).bind(&target.normalized).fetch_optional(&mut *tx).await?;
    if let Some(job) = active {
        return Ok((
            StatusCode::ACCEPTED,
            Json(json!({"id":job,"status":"queued"})),
        ));
    }
    let pending: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM package_comparisons WHERE status IN ('queued','running')",
    )
    .fetch_one(&mut *tx)
    .await?;
    if pending >= 10 {
        return Err(ApiError(
            StatusCode::TOO_MANY_REQUESTS,
            "Comparison queue is full; retry later".into(),
        ));
    }
    let job = Uuid::new_v4();
    sqlx::query("INSERT INTO package_comparisons(id,package_id,baseline_version,target_version,inputs) VALUES($1,$2,$3,$4,$5)").bind(job).bind(&id).bind(baseline.normalized).bind(target.normalized).bind(json!(inputs)).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"id":job,"status":"queued"})),
    ))
}
pub async fn list(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    let id = package_id(&id)?;
    let reader = reader(&state, &headers).await?;
    crate::feeds::check_package(&state, &id)?;
    if !id.starts_with(&reader.prefix) {
        return Err(ApiError::forbidden("Package is outside this key's prefix"));
    }
    if !crate::upstream::cache::local_exists(&state, &id).await? {
        crate::upstream::cache::pinned(&state, &reader, &id).await?;
    }
    let rows:Vec<Value>=sqlx::query_scalar("SELECT jsonb_build_object('id',id,'baselineVersion',baseline_version,'targetVersion',target_version,'status',status,'createdAt',created_at,'completedAt',completed_at) FROM package_comparisons WHERE package_id=$1 ORDER BY created_at DESC LIMIT 30").bind(&id).fetch_all(&state.db).await?;
    Ok(Json(json!(rows)))
}
pub async fn details(
    State(state): State<AppState>,
    Path((id, job)): Path<(String, Uuid)>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    let id = package_id(&id)?;
    let reader = reader(&state, &headers).await?;
    let row: Option<Value> = sqlx::query_scalar(
        "SELECT to_jsonb(c) FROM package_comparisons c WHERE id=$1 AND package_id=$2",
    )
    .bind(job)
    .bind(&id)
    .fetch_optional(&state.db)
    .await?;
    let mut row = row.ok_or_else(ApiError::not_found)?;
    let inputs: [Reference; 2] =
        serde_json::from_value(row["inputs"].clone()).map_err(ApiError::internal)?;
    reference::authorize(
        &state,
        &reader,
        &id,
        row["baseline_version"].as_str().unwrap_or_default(),
        row["target_version"].as_str().unwrap_or_default(),
        &inputs,
    )
    .await?;
    for key in ["inputs", "lease_token", "lease_until"] {
        row.as_object_mut().unwrap().remove(key);
    }
    Ok(Json(row))
}
