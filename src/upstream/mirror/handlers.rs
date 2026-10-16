use super::{model::Settings, repository};
use crate::{
    AppState,
    error::{ApiError, ApiResult},
    upstream::sources,
};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;
async fn exists(state: &AppState, id: Uuid, create: bool) -> ApiResult<()> {
    let found: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM upstream_sources WHERE id=$1)")
            .bind(id)
            .fetch_one(&state.db)
            .await?;
    if !found {
        return Err(ApiError::not_found());
    }
    if create {
        sqlx::query("INSERT INTO upstream_mirrors(source_id) VALUES($1) ON CONFLICT DO NOTHING")
            .bind(id)
            .execute(&state.db)
            .await?;
    }
    Ok(())
}
pub async fn status(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    sources::admin(&state, &headers)?;
    exists(&state, id, false).await?;
    Ok(Json(repository::status(&state.db, id).await?))
}
pub async fn settings(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(input): Json<Settings>,
) -> ApiResult<Json<Value>> {
    sources::admin(&state, &headers)?;
    if !(60..=86400).contains(&input.interval_seconds) {
        return Err(ApiError::bad("Mirror interval must be 60–86400 seconds"));
    }
    exists(&state, id, true).await?;
    sqlx::query("UPDATE upstream_mirrors SET enabled=$2,interval_seconds=$3,next_run=CASE WHEN NOT enabled AND $2 THEN now() ELSE next_run END WHERE source_id=$1").bind(id).bind(input.enabled).bind(input.interval_seconds).execute(&state.db).await?;
    Ok(Json(repository::status(&state.db, id).await?))
}
pub async fn request(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> ApiResult<(StatusCode, Json<Value>)> {
    sources::admin(&state, &headers)?;
    exists(&state, id, true).await?;
    let enabled: bool = sqlx::query_scalar("SELECT enabled FROM upstream_sources WHERE id=$1")
        .bind(id)
        .fetch_one(&state.db)
        .await?;
    if !enabled {
        return Err(ApiError::bad(
            "Enable the source before requesting a mirror",
        ));
    }
    sqlx::query("UPDATE upstream_mirrors SET requested=true,status=CASE WHEN status='running' THEN status ELSE 'queued' END,next_run=now() WHERE source_id=$1").bind(id).execute(&state.db).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(repository::status(&state.db, id).await?),
    ))
}
pub async fn cancel(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    sources::admin(&state, &headers)?;
    exists(&state, id, true).await?;
    sqlx::query("UPDATE upstream_mirrors SET requested=false,enabled=false,status='paused' WHERE source_id=$1").bind(id).execute(&state.db).await?;
    Ok(Json(repository::status(&state.db, id).await?))
}
#[derive(Deserialize)]
pub struct Jobs {
    #[serde(default)]
    skip: i64,
}
pub async fn jobs(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Query(query): Query<Jobs>,
) -> ApiResult<Json<Value>> {
    sources::admin(&state, &headers)?;
    if !(0..=1000000).contains(&query.skip) {
        return Err(ApiError::bad("Invalid job offset"));
    }
    let jobs:Vec<Value>=sqlx::query_scalar("SELECT jsonb_build_object('id',package_id,'version',version,'action',action,'done',done,'skipped',skipped,'attempts',attempts,'lastError',last_error,'nextAttempt',next_attempt) FROM upstream_mirror_jobs WHERE source_id=$1 ORDER BY done,attempts DESC,package_id,version LIMIT 100 OFFSET $2").bind(id).bind(query.skip).fetch_all(&state.db).await?;
    Ok(Json(json!(jobs)))
}
