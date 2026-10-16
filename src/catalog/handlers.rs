use super::{protocol, repository};
use crate::{
    AppState,
    error::{ApiError, ApiResult},
};
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde_json::Value;
use sha2::{Digest, Sha256};
fn response(value: Value, headers: &HeaderMap) -> Response {
    let etag = format!(
        "\"{}\"",
        hex::encode(Sha256::digest(value.to_string().as_bytes()))
    );
    let matched = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|values| {
            values.split(',').any(|v| {
                let v = v.trim();
                v == "*" || v.trim_start_matches("W/") == etag
            })
        });
    let mut result = if matched {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        Json(value).into_response()
    };
    result
        .headers_mut()
        .insert(header::ETAG, etag.parse().unwrap());
    result
        .headers_mut()
        .insert(header::CACHE_CONTROL, "private, no-cache".parse().unwrap());
    result.headers_mut().insert(
        header::VARY,
        "Authorization, X-NuGet-ApiKey".parse().unwrap(),
    );
    result
}
pub async fn index(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Response> {
    let pages = repository::pages(&state).await?;
    Ok(response(protocol::index(&state, &pages), &headers))
}
pub async fn page(
    State(state): State<AppState>,
    Path(file): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let number = file
        .strip_suffix(".json")
        .and_then(|s| s.parse::<i64>().ok())
        .filter(|n| *n >= 0)
        .ok_or_else(ApiError::not_found)?;
    let events = repository::page(&state, number).await?;
    if events.is_empty() {
        return Err(ApiError::not_found());
    }
    Ok(response(protocol::page(&state, number, &events), &headers))
}
pub async fn leaf(
    State(state): State<AppState>,
    Path(file): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let id = file
        .strip_suffix(".json")
        .and_then(|s| uuid::Uuid::parse_str(s).ok())
        .ok_or_else(ApiError::not_found)?;
    let event = repository::leaf(&state, id).await?;
    Ok(response(protocol::leaf(&state, &event), &headers))
}
