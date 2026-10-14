//! Locally mirrored NuGet VulnerabilityInfo, with atomic and cacheable snapshots.
use crate::{
    AppState, auth,
    error::{ApiError, ApiResult},
    version::{NuGetVersion, package_id},
};
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::time::Duration;

fn range_bounds(text: &str) -> ApiResult<(Option<String>, bool, Option<String>, bool)> {
    let text = text.trim();
    let parse = |s: &str| -> ApiResult<Option<String>> {
        if s.trim().is_empty() {
            Ok(None)
        } else {
            Ok(Some(NuGetVersion::parse(s.trim())?.sort_key))
        }
    };
    if text.starts_with('[') || text.starts_with('(') {
        if !(text.ends_with(']') || text.ends_with(')')) {
            return Err(ApiError::bad("Invalid vulnerability version range"));
        }
        let inner = &text[1..text.len() - 1];
        let (a, b) = if let Some((a, b)) = inner.split_once(',') {
            (a, b)
        } else if text.starts_with('[') && text.ends_with(']') {
            (inner, inner)
        } else {
            return Err(ApiError::bad("Invalid exact version range"));
        };
        let low = parse(a)?;
        let high = parse(b)?;
        if low.as_ref().zip(high.as_ref()).is_some_and(|(a, b)| a > b) {
            return Err(ApiError::bad("Reversed vulnerability range"));
        }
        Ok((low, text.starts_with('['), high, text.ends_with(']')))
    } else {
        Ok((Some(NuGetVersion::parse(text)?.sort_key), true, None, false))
    }
}
pub fn affected(range: &str, version: &str) -> bool {
    let Ok((low, li, high, hi)) = range_bounds(range) else {
        return false;
    };
    let Ok(v) = NuGetVersion::parse(version) else {
        return false;
    };
    low.as_ref()
        .is_none_or(|b| v.sort_key > *b || (li && v.sort_key == *b))
        && high
            .as_ref()
            .is_none_or(|b| v.sort_key < *b || (hi && v.sort_key == *b))
}
fn validate(data: &Value) -> ApiResult<()> {
    if data.as_array().is_some_and(Vec::is_empty) {
        return Ok(());
    }
    let map = data
        .as_object()
        .ok_or_else(|| ApiError::bad("Vulnerability data must be an object or empty array"))?;
    for (id, items) in map {
        if package_id(id)? != *id {
            return Err(ApiError::bad("Advisory package IDs must be lowercase"));
        }
        let items = items
            .as_array()
            .ok_or_else(|| ApiError::bad("Advisories must be arrays"))?;
        if items.len() > 1000 {
            return Err(ApiError::bad("Too many advisories per package"));
        }
        for a in items {
            if a["severity"].as_u64().is_none_or(|s| s > 3) {
                return Err(ApiError::bad("Advisory severity must be 0–3"));
            }
            let url = url::Url::parse(a["url"].as_str().unwrap_or_default())
                .map_err(|_| ApiError::bad("Invalid advisory URL"))?;
            if url.scheme() != "https"
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
            {
                return Err(ApiError::bad(
                    "Advisory links must use HTTPS without credentials",
                ));
            }
            range_bounds(
                a["versions"]
                    .as_str()
                    .ok_or_else(|| ApiError::bad("Advisory version range required"))?,
            )?;
        }
    }
    Ok(())
}
pub async fn available(state: &AppState) -> ApiResult<bool> {
    Ok(
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM public.audit_snapshots WHERE current)")
            .fetch_one(&state.db)
            .await?,
    )
}
pub async fn index(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let row: Option<(String, chrono::DateTime<chrono::Utc>)> =
        sqlx::query_as("SELECT hash,created_at FROM public.audit_snapshots WHERE current")
            .fetch_optional(&state.db)
            .await?;
    let (hash, time) = row.ok_or_else(ApiError::not_found)?;
    Ok(Json(
        json!([{"@name":"base","@id":format!("{}/v3/vulnerabilities/{hash}.json",state.config.public_url),"@updated":time,"comment":"NuShelf locally mirrored vulnerability data"}]),
    ))
}
pub async fn page(State(state): State<AppState>, Path(file): Path<String>) -> ApiResult<Response> {
    let hash = file.strip_suffix(".json").ok_or_else(ApiError::not_found)?;
    let data: Value = sqlx::query_scalar("SELECT data FROM public.audit_snapshots WHERE hash=$1")
        .bind(hash)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(ApiError::not_found)?;
    Ok((
        [(header::CACHE_CONTROL, "private, max-age=86400")],
        Json(data),
    )
        .into_response())
}
fn unbounded_first(a: &Option<String>, b: &Option<String>) -> std::cmp::Ordering {
    match (a, b) {
        (None, Some(_)) => std::cmp::Ordering::Less,
        (Some(_), None) => std::cmp::Ordering::Greater,
        _ => b.cmp(a),
    }
}
async fn store(state: &AppState, mut data: Value) -> ApiResult<String> {
    validate(&data)?;
    if let Some(map) = data.as_object_mut() {
        for items in map.values_mut() {
            if let Some(items) = items.as_array_mut() {
                items.sort_by(|a, b| {
                    let ar = range_bounds(a["versions"].as_str().unwrap()).unwrap();
                    let br = range_bounds(b["versions"].as_str().unwrap()).unwrap();
                    unbounded_first(&ar.2, &br.2)
                        .then(unbounded_first(&ar.0, &br.0))
                        .then(a["url"].as_str().cmp(&b["url"].as_str()))
                });
                items.dedup();
            }
        }
        if map.is_empty() {
            data = json!([]);
        }
    }
    let hash = hex::encode(Sha256::digest(
        serde_json::to_vec(&data).map_err(ApiError::internal)?,
    ));
    let mut tx = state.db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(314159266)")
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE public.audit_snapshots SET current=false WHERE current")
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO public.audit_snapshots(hash,data,current) VALUES($1,$2,true) ON CONFLICT(hash) DO UPDATE SET current=true").bind(&hash).bind(data).execute(&mut *tx).await?;
    sqlx::query(
        "UPDATE public.audit_settings SET last_success=now(),last_error=NULL WHERE singleton",
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM public.audit_snapshots WHERE NOT current AND created_at < now()-interval '7 days'").execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(hash)
}
pub async fn status(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    admin(&state, &headers)?;
    let row:Value=sqlx::query_scalar("SELECT jsonb_build_object('enabled',enabled,'lastAttempt',last_attempt,'lastSuccess',last_success,'lastError',last_error,'packageCount',COALESCE((SELECT CASE WHEN jsonb_typeof(data)='object' THEN (SELECT count(*) FROM jsonb_object_keys(data)) ELSE 0 END FROM public.audit_snapshots WHERE current),0)) FROM public.audit_settings").fetch_one(&state.db).await?;
    Ok(Json(row))
}
fn admin(state: &AppState, headers: &HeaderMap) -> ApiResult<()> {
    if !auth::is_admin(state, headers) {
        return Err(ApiError::unauthorized());
    }
    Ok(())
}
pub async fn settings(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> ApiResult<StatusCode> {
    admin(&state, &headers)?;
    let enabled = input["enabled"]
        .as_bool()
        .ok_or_else(|| ApiError::bad("enabled must be a boolean"))?;
    sqlx::query("UPDATE public.audit_settings SET enabled=$1 WHERE singleton")
        .bind(enabled)
        .execute(&state.db)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
pub async fn import(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(data): Json<Value>,
) -> ApiResult<Json<Value>> {
    admin(&state, &headers)?;
    Ok(Json(json!({"hash":store(&state,data).await?})))
}
async fn fetch(client: &reqwest::Client, url: &str, max: usize) -> ApiResult<Value> {
    let url = url::Url::parse(url).map_err(ApiError::internal)?;
    if url.scheme() != "https"
        || !matches!(url.host_str(), Some("data.nuget.org" | "api.nuget.org"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
    {
        return Err(ApiError::bad("Unexpected NuGet audit resource URL"));
    }
    let mut response = client
        .get(url)
        .send()
        .await
        .map_err(ApiError::internal)?
        .error_for_status()
        .map_err(ApiError::internal)?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(ApiError::internal)? {
        if bytes.len() + chunk.len() > max {
            return Err(ApiError::bad("Audit response exceeds size limit"));
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| ApiError::bad("Invalid audit JSON"))
}
async fn synchronize(state: &AppState) -> ApiResult<String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()
        .map_err(ApiError::internal)?;
    let service = fetch(&client, "https://data.nuget.org/v3/index.json", 1024 * 1024).await?;
    let resource = service["resources"]
        .as_array()
        .and_then(|rs| rs.iter().find(|r| r["@type"] == "VulnerabilityInfo/6.7.0"))
        .and_then(|r| r["@id"].as_str())
        .ok_or_else(|| ApiError::bad("NuGet did not advertise vulnerability data"))?;
    let index = fetch(&client, resource, 1024 * 1024).await?;
    let pages = index
        .as_array()
        .filter(|p| !p.is_empty() && p.len() <= 16)
        .ok_or_else(|| ApiError::bad("Invalid vulnerability index"))?;
    let mut merged = serde_json::Map::new();
    let mut total = 0;
    for p in pages {
        let page = fetch(
            &client,
            p["@id"].as_str().unwrap_or_default(),
            64 * 1024 * 1024,
        )
        .await?;
        validate(&page)?;
        total += serde_json::to_vec(&page).map_err(ApiError::internal)?.len();
        if total > 128 * 1024 * 1024 {
            return Err(ApiError::bad("Combined audit data exceeds size limit"));
        }
        if let Some(map) = page.as_object() {
            for (id, items) in map {
                merged
                    .entry(id.clone())
                    .or_insert_with(|| json!([]))
                    .as_array_mut()
                    .unwrap()
                    .extend(items.as_array().unwrap().iter().cloned());
            }
        }
    }
    store(state, Value::Object(merged)).await
}
pub async fn run_sync(state: &AppState) -> ApiResult<String> {
    if state.db.options().get_max_connections() < 2 {
        return Err(ApiError::bad(
            "Audit synchronization requires at least two database connections",
        ));
    }
    let mut lock = state.db.begin().await?;
    let locked: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(314159267)")
        .fetch_one(&mut *lock)
        .await?;
    if !locked {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "Audit synchronization already running".into(),
        ));
    }
    sqlx::query("UPDATE public.audit_settings SET last_attempt=now() WHERE singleton")
        .execute(&state.db)
        .await?;
    let result = tokio::time::timeout(Duration::from_secs(60), synchronize(state))
        .await
        .unwrap_or_else(|_| {
            Err(ApiError(
                StatusCode::GATEWAY_TIMEOUT,
                "Audit synchronization timed out".into(),
            ))
        });
    if let Err(e) = &result {
        sqlx::query("UPDATE public.audit_settings SET last_error=$1 WHERE singleton")
            .bind(&e.1)
            .execute(&state.db)
            .await?;
    }
    lock.commit().await?;
    result
}
pub async fn sync(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    admin(&state, &headers)?;
    Ok(Json(json!({"hash":run_sync(&state).await?})))
}
pub fn worker(state: AppState) {
    tokio::spawn(async move {
        loop {
            let enabled = sqlx::query_scalar::<_, bool>(
                "SELECT enabled FROM public.audit_settings WHERE singleton",
            )
            .fetch_one(&state.db)
            .await
            .unwrap_or(false);
            if enabled && let Err(e) = run_sync(&state).await {
                tracing::warn!(error=%e.1,"Audit sync failed; retaining previous snapshot");
            }
            tokio::time::sleep(Duration::from_secs(3600)).await;
        }
    });
}
pub async fn advisories(state: &AppState, id: &str) -> ApiResult<Value> {
    let row: Option<Value> =
        sqlx::query_scalar("SELECT COALESCE(data->$1,'[]'::jsonb) FROM public.audit_snapshots WHERE current")
            .bind(id.to_ascii_lowercase())
            .fetch_optional(&state.db)
            .await?;
    Ok(row.filter(|v| v.is_array()).unwrap_or_else(|| json!([])))
}
pub async fn decorate(state: &AppState, result: &mut Value) -> ApiResult<()> {
    let id = result["id"].as_str().unwrap_or_default();
    let advisories = advisories(state, id).await?;
    if let Some(versions) = result["versions"].as_array_mut() {
        for version in versions {
            let v = version["version"].as_str().unwrap_or_default();
            let items: Vec<_> = advisories
                .as_array()
                .unwrap()
                .iter()
                .filter(|a| affected(a["versions"].as_str().unwrap_or_default(), v))
                .cloned()
                .collect();
            version["vulnerabilities"] = json!(items);
        }
    }
    result["advisories"] = advisories;
    Ok(())
}
pub async fn package(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    let id = package_id(&id)?;
    crate::feeds::check_package(&state, &id)?;
    Ok(Json(advisories(&state, &id).await?))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ranges() {
        assert!(affected("(,13.0.1)", "12.0.3"));
        assert!(!affected("(,13.0.1)", "13.0.1"));
        assert!(affected("[1.0.0,2.0.0)", "1.0.0"));
        assert!(!affected("[1.0.0]", "1.0.1"));
        assert!(!affected("(2.0.0,1.0.0)", "1.5.0"));
    }
    #[test]
    fn rejects_bad_advisories() {
        assert!(
            validate(
                &json!({"a":[{"severity":4,"versions":"(,1.0.0)","url":"https://example.com"}]})
            )
            .is_err()
        );
        assert!(
            validate(
                &json!({"a":[{"severity":2,"versions":"(,1.0.0)","url":"javascript:alert(1)"}]})
            )
            .is_err()
        );
    }
}
