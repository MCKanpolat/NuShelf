//! Declared reverse dependencies, indexed transactionally at publication time.
use crate::{
    AppState,
    error::{ApiError, ApiResult},
    upstream::sources::Reader,
    version::package_id,
};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::HeaderMap,
};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    #[serde(default)]
    skip: i64,
    take: Option<i64>,
    #[serde(default)]
    include_unlisted: bool,
}
pub async fn list(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Query(page): Query<Page>,
) -> ApiResult<Json<Value>> {
    let id = package_id(&id)?;
    crate::feeds::check_package(&state, &id)?;
    let reader = Reader::from_headers(&state, &headers).await?;
    if !id.starts_with(&reader.prefix) {
        return Err(ApiError::forbidden("Package is outside this key's prefix"));
    }
    let take = page.take.unwrap_or(50);
    if !(1..=200).contains(&take) || !(0..=100000).contains(&page.skip) {
        return Err(ApiError::bad("Invalid dependent pagination"));
    }
    // One repeatable snapshot keeps the count and page consistent during pushes.
    let mut tx = state.db.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await?;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM package_dependencies d JOIN package_versions v USING(package_id,version) WHERE dependency_id=$1 AND (v.listed OR $2) AND starts_with(d.package_id,$3) AND starts_with(d.package_id,$4)")
        .bind(&id).bind(page.include_unlisted).bind(&reader.prefix).bind(&state.read_prefix).fetch_one(&mut *tx).await?;
    let rows: Vec<Value> = sqlx::query_scalar(include_str!("list.sql"))
        .bind(&id)
        .bind(page.include_unlisted)
        .bind(&reader.prefix)
        .bind(&state.read_prefix)
        .bind(take)
        .bind(page.skip)
        .fetch_all(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(
        json!({"packageId":id,"totalHits":count,"skip":page.skip,"take":take,"items":rows,"scope":"local","relationship":"declared"}),
    ))
}
