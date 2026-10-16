use super::model::Query as SearchQuery;
use crate::{AppState, error::ApiResult, upstream::sources::Reader};
use axum::{
    Json,
    extract::{Query, State},
    http::HeaderMap,
};
use serde_json::Value;
pub async fn search(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<SearchQuery>,
) -> ApiResult<Json<Value>> {
    let reader = Reader::from_headers(&state, &headers).await?;
    Ok(Json(super::service::search(&state, &reader, &query).await?))
}
