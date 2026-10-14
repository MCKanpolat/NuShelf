use crate::{
    AppState,
    error::{ApiError, ApiResult},
    version::{NuGetVersion, package_id},
};
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, header},
    response::{IntoResponse, Response},
};
use base64::Engine;
use serde_json::{Value, json};
pub fn markdown(source: &str) -> String {
    let parser = pulldown_cmark::Parser::new_ext(
        source,
        pulldown_cmark::Options::ENABLE_TABLES
            | pulldown_cmark::Options::ENABLE_STRIKETHROUGH
            | pulldown_cmark::Options::ENABLE_TASKLISTS,
    );
    let mut html = String::new();
    pulldown_cmark::html::push_html(&mut html, parser);
    let mut clean = ammonia::Builder::default();
    clean.url_schemes(["https", "http", "mailto"].into_iter().collect());
    clean
        .rm_tags(["img", "style"])
        .link_rel(Some("noopener noreferrer"));
    clean.clean(&html).to_string()
}
pub async fn load(
    state: &AppState,
    headers: &HeaderMap,
    id: &str,
    version: &str,
) -> ApiResult<Value> {
    let id = package_id(id)?;
    let version = NuGetVersion::parse(version)?.normalized;
    crate::feeds::check_package(state, &id)?;
    let (key, mut assets) = match sqlx::query_as::<_, (String, Value)>(
        "SELECT storage_key,assets FROM package_versions WHERE package_id=$1 AND version=$2",
    )
    .bind(&id)
    .bind(&version)
    .fetch_optional(&state.db)
    .await?
    {
        Some(row) => row,
        None => {
            let reader = crate::upstream::sources::Reader::from_headers(state, headers).await?;
            let (cached, _, _) =
                crate::upstream::cache::ensure(state, &reader, &id, &version).await?;
            let assets: Value = sqlx::query_scalar(
                "SELECT assets FROM upstream_cache WHERE package_id=$1 AND version=$2",
            )
            .bind(&id)
            .bind(&version)
            .fetch_one(&state.db)
            .await?;
            (cached.storage_key, assets)
        }
    };
    if assets.as_object().is_none_or(|o| o.is_empty()) {
        let result = state
            .storage
            .get(&object_store::path::Path::from(key.as_str()))
            .await?;
        if result.meta.size > state.config.max_package_bytes as u64 {
            return Err(ApiError::bad("Package exceeds asset extraction size limit"));
        }
        let bytes = result.bytes().await?;
        assets = tokio::task::spawn_blocking(move || crate::package::parse(&bytes))
            .await
            .map_err(ApiError::internal)??
            .assets;
        sqlx::query("UPDATE package_versions SET assets=$3 WHERE package_id=$1 AND version=$2")
            .bind(&id)
            .bind(&version)
            .bind(&assets)
            .execute(&state.db)
            .await?;
        sqlx::query("UPDATE upstream_cache SET assets=$3 WHERE package_id=$1 AND version=$2")
            .bind(&id)
            .bind(&version)
            .bind(&assets)
            .execute(&state.db)
            .await?;
    }
    Ok(assets)
}
pub async fn details(
    State(state): State<AppState>,
    Path((id, version)): Path<(String, String)>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    let assets = load(&state, &headers, &id, &version).await?;
    let html = assets["readme"].as_str().map(markdown);
    let icon = assets["icon"].as_str().map(|_| {
        format!(
            "{}/api/packages/{}/{}/icon",
            state.config.public_url,
            id.to_ascii_lowercase(),
            version
        )
    });
    Ok(Json(
        json!({"readmeHtml":html,"iconUrl":icon,"frameworks":assets["frameworks"],"repository":assets["repository"]}),
    ))
}
pub async fn icon(
    State(state): State<AppState>,
    Path((id, version)): Path<(String, String)>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let assets = load(&state, &headers, &id, &version).await?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(assets["icon"].as_str().ok_or_else(ApiError::not_found)?)
        .map_err(ApiError::internal)?;
    let mime = assets["iconType"]
        .as_str()
        .ok_or_else(ApiError::not_found)?;
    Ok((
        [
            (header::CONTENT_TYPE, mime),
            (header::CACHE_CONTROL, "private, max-age=3600"),
        ],
        bytes,
    )
        .into_response())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sanitizes_untrusted_markdown() {
        let html = markdown(
            "# Test\n<script>alert(1)</script>\n[bad](javascript:alert(1))\n<img src=x onerror=alert(1)>\n**safe**",
        );
        assert!(html.contains("<strong>safe</strong>"));
        assert!(!html.contains("<script"));
        assert!(!html.contains("javascript:"));
        assert!(!html.contains("onerror"));
    }
}
