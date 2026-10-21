use crate::{
    AppState,
    error::{ApiError, ApiResult},
    upstream::sources::admin,
};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Policy {
    enabled: bool,
    interval_hours: i32,
}
pub async fn status(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    admin(&state, &headers)?;
    let policy:Value=sqlx::query_scalar("SELECT jsonb_build_object('enabled',enabled,'intervalHours',interval_hours,'nextScan',next_scan) FROM integrity_policy WHERE singleton").fetch_one(&state.db).await?;
    let runs:Vec<Value>=sqlx::query_scalar("SELECT jsonb_build_object('id',r.id,'status',r.status,'createdAt',r.created_at,'startedAt',r.started_at,'completedAt',r.completed_at,'counts',(SELECT jsonb_object_agg(status,n) FROM (SELECT status,count(*) n FROM integrity_items WHERE run_id=r.id GROUP BY status) c)) FROM integrity_runs r ORDER BY r.created_at DESC,r.id LIMIT 20").fetch_all(&state.db).await?;
    Ok(Json(json!({"policy":policy,"runs":runs})))
}
pub async fn update(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<Policy>,
) -> ApiResult<StatusCode> {
    admin(&state, &headers)?;
    if !(1..=8760).contains(&input.interval_hours) {
        return Err(ApiError::bad("Scan interval must be 1–8760 hours"));
    }
    sqlx::query("UPDATE integrity_policy SET enabled=$1,interval_hours=$2,next_scan=CASE WHEN $1 AND NOT enabled THEN now() ELSE LEAST(next_scan,now()+($2*interval '1 hour')) END WHERE singleton")
        .bind(input.enabled).bind(input.interval_hours).execute(&state.db).await?;
    Ok(StatusCode::NO_CONTENT)
}
pub async fn start(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<(StatusCode, Json<Value>)> {
    admin(&state, &headers)?;
    let id = super::repository::enqueue(&state, false)
        .await?
        .expect("manual enqueue returns the active run");
    Ok((StatusCode::ACCEPTED, Json(json!({"id":id}))))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    #[serde(default)]
    skip: i64,
    take: Option<i64>,
    #[serde(default)]
    show_all: bool,
}
pub async fn details(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Query(page): Query<Page>,
) -> ApiResult<Json<Value>> {
    admin(&state, &headers)?;
    let take = page.take.unwrap_or(100);
    if !(1..=200).contains(&take) || !(0..=1000000).contains(&page.skip) {
        return Err(ApiError::bad("Invalid integrity pagination"));
    }
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM integrity_runs WHERE id=$1)")
            .bind(id)
            .fetch_one(&state.db)
            .await?;
    if !exists {
        return Err(ApiError::not_found());
    }
    let rows:Vec<Value>=sqlx::query_scalar("SELECT jsonb_build_object('packageId',package_id,'version',version,'kind',kind,'status',status,'expectedSize',expected_size,'actualSize',actual_size,'expectedDigest',expected_digest,'actualDigest',actual_digest,'checkedAt',checked_at) FROM integrity_items WHERE run_id=$1 AND ($2 OR status IN ('missing','mismatch','unavailable')) ORDER BY storage_key LIMIT $3 OFFSET $4")
        .bind(id).bind(page.show_all).bind(take).bind(page.skip).fetch_all(&state.db).await?;
    Ok(Json(
        json!({"id":id,"items":rows,"take":take,"skip":page.skip}),
    ))
}
