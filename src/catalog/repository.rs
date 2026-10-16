use super::model::{Event, PAGE_SIZE, PageSummary};
use crate::{
    AppState,
    error::{ApiError, ApiResult},
};
use uuid::Uuid;
pub(super) async fn pages(state: &AppState) -> ApiResult<Vec<PageSummary>> {
    Ok(sqlx::query_as(include_str!("sql/pages.sql"))
        .bind(&state.read_prefix)
        .bind(PAGE_SIZE)
        .fetch_all(&state.db)
        .await?)
}
pub(super) async fn page(state: &AppState, page: i64) -> ApiResult<Vec<Event>> {
    let start = page
        .checked_mul(PAGE_SIZE)
        .and_then(|n| n.checked_add(1))
        .ok_or_else(ApiError::not_found)?;
    let end = start
        .checked_add(PAGE_SIZE)
        .ok_or_else(ApiError::not_found)?;
    Ok(sqlx::query_as("SELECT * FROM catalog_events WHERE sequence>=$1 AND sequence<$2 AND starts_with(package_id,$3) ORDER BY sequence").bind(start).bind(end).bind(&state.read_prefix).fetch_all(&state.db).await?)
}
pub(super) async fn leaf(state: &AppState, id: Uuid) -> ApiResult<Event> {
    sqlx::query_as("SELECT * FROM catalog_events WHERE event_id=$1 AND starts_with(package_id,$2)")
        .bind(id)
        .bind(&state.read_prefix)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(ApiError::not_found)
}
