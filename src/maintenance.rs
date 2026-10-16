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
use chrono::{DateTime, Utc};
use futures_util::TryStreamExt;
use object_store::path::Path as ObjectPath;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::FromRow;
use std::time::Duration;
#[derive(FromRow, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Policy {
    enabled: bool,
    max_bytes: i64,
    idle_days: i32,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    #[serde(default = "dry")]
    dry_run: bool,
    #[serde(default)]
    orphans: bool,
}
fn dry() -> bool {
    true
}
fn admin(state: &AppState, headers: &HeaderMap) -> ApiResult<()> {
    if !auth::is_admin(state, headers) {
        return Err(ApiError::unauthorized());
    }
    Ok(())
}
pub async fn policy(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    admin(&state, &headers)?;
    let v:Value=sqlx::query_scalar("SELECT jsonb_build_object('enabled',enabled,'maxBytes',max_bytes,'idleDays',idle_days,'lastRun',last_run,'lastResult',last_result,'usedBytes',(SELECT COALESCE(sum(size),0) FROM upstream_cache),'pendingCleanup',(SELECT count(*) FROM storage_cleanup)) FROM cache_policy").fetch_one(&state.db).await?;
    Ok(Json(v))
}
pub async fn settings(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<Policy>,
) -> ApiResult<StatusCode> {
    admin(&state, &headers)?;
    if input.max_bytes < 0 || !(0..=3650).contains(&input.idle_days) {
        return Err(ApiError::bad(
            "Cache quota must be nonnegative and idle days must be 0–3650",
        ));
    }
    sqlx::query("UPDATE cache_policy SET enabled=$1,max_bytes=$2,idle_days=$3 WHERE singleton")
        .bind(input.enabled)
        .bind(input.max_bytes)
        .bind(input.idle_days)
        .execute(&state.db)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
pub async fn protect(
    State(state): State<AppState>,
    Path((id, version)): Path<(String, String)>,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> ApiResult<StatusCode> {
    admin(&state, &headers)?;
    let protected = input["protected"]
        .as_bool()
        .ok_or_else(|| ApiError::bad("protected must be a boolean"))?;
    let result =
        sqlx::query("UPDATE upstream_cache SET protected=$3 WHERE package_id=$1 AND version=$2")
            .bind(package_id(&id)?)
            .bind(NuGetVersion::parse(&version)?.normalized)
            .bind(protected)
            .execute(&state.db)
            .await?;
    if result.rows_affected() == 0 {
        return Err(ApiError::not_found());
    }
    Ok(StatusCode::NO_CONTENT)
}
#[derive(FromRow)]
struct Candidate {
    package_id: String,
    version: String,
    storage_key: String,
    size: i64,
    last_accessed_at: DateTime<Utc>,
}
async fn referenced(state: &AppState, key: &str) -> ApiResult<bool> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM package_versions WHERE storage_key=$1 UNION ALL SELECT 1 FROM upstream_cache WHERE storage_key=$1 UNION ALL SELECT 1 FROM symbol_files WHERE storage_key=$1)").bind(key).fetch_one(&state.db).await?)
}
pub async fn execute(state: &AppState, dry_run: bool, orphans: bool) -> ApiResult<Value> {
    let mut tx = state.db.begin().await?;
    let locked: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(314159269)")
        .fetch_one(&mut *tx)
        .await?;
    if !locked {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "Cache maintenance is already running".into(),
        ));
    }
    let policy: Policy =
        sqlx::query_as("SELECT enabled,max_bytes,idle_days FROM cache_policy WHERE singleton")
            .fetch_one(&mut *tx)
            .await?;
    let used: i64 = sqlx::query_scalar("SELECT COALESCE(sum(size),0)::bigint FROM upstream_cache")
        .fetch_one(&mut *tx)
        .await?;
    let candidates:Vec<Candidate>=sqlx::query_as("SELECT package_id,version,storage_key,size,last_accessed_at FROM upstream_cache WHERE NOT protected AND cached_at<now()-interval '1 hour' ORDER BY last_accessed_at,package_id,version LIMIT 500").fetch_all(&mut *tx).await?;
    let mut remaining = used;
    let mut selected = Vec::new();
    let mut ids = Vec::new();
    for c in candidates {
        let idle = policy.idle_days > 0
            && c.last_accessed_at < Utc::now() - chrono::Duration::days(policy.idle_days as i64);
        let over = policy.max_bytes > 0 && remaining > policy.max_bytes;
        if !idle && !over {
            continue;
        }
        if !dry_run {
            // The source lock serializes eviction with cache fills and manual purges.
            let source: Option<uuid::Uuid> =
                sqlx::query_scalar("SELECT source_id FROM upstream_metadata WHERE package_id=$1")
                    .bind(&c.package_id)
                    .fetch_optional(&mut *tx)
                    .await?;
            if let Some(source) = source {
                sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
                    .bind(format!("source:{source}"))
                    .execute(&mut *tx)
                    .await?;
            }
            let deleted:Option<String>=sqlx::query_scalar("DELETE FROM upstream_cache WHERE package_id=$1 AND version=$2 AND NOT protected AND last_accessed_at=$3 RETURNING storage_key").bind(&c.package_id).bind(&c.version).bind(c.last_accessed_at).fetch_optional(&mut *tx).await?;
            if deleted.is_none() {
                continue;
            }
            sqlx::query(
                "INSERT INTO storage_cleanup(storage_key) VALUES($1) ON CONFLICT DO NOTHING",
            )
            .bind(&c.storage_key)
            .execute(&mut *tx)
            .await?;
            ids.push(c.package_id.clone());
        }
        remaining -= c.size;
        selected.push(json!({"id":c.package_id,"version":c.version,"size":c.size}));
    }
    if !dry_run {
        ids.sort();
        ids.dedup();
        for id in ids {
            sqlx::query("SELECT refresh_upstream_search($1)")
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
    }
    tx.commit().await?;
    let mut orphan_keys = Vec::new();
    let mut scanned = 0;
    let mut more_objects = false;
    if orphans {
        let previous: Option<String> = sqlx::query_scalar("SELECT orphan_cursor FROM cache_policy")
            .fetch_one(&state.db)
            .await?;
        let mut stream = if let Some(previous) = previous {
            state
                .storage
                .list_with_offset(None, &ObjectPath::from(previous))
        } else {
            state.storage.list(None)
        };
        // Providers need not return sorted objects. Keep the smallest 1000 paths
        // across the complete listing before advancing a lexical cursor.
        let mut batch = std::collections::BTreeMap::new();
        let mut total = 0;
        while let Some(meta) = stream.try_next().await? {
            let key = meta.location.to_string();
            if state.feed.is_none() && key.starts_with("feeds/") {
                continue;
            }
            total += 1;
            batch.insert(key, meta);
            if batch.len() > 1000 {
                batch.pop_last();
            }
        }
        more_objects = total > 1000;
        let cursor = batch.last_key_value().map(|(key, _)| key.clone());
        for (key, meta) in batch {
            scanned += 1;
            if (key.ends_with(".nupkg") || key.ends_with(".pdb"))
                && meta.last_modified < Utc::now() - chrono::Duration::hours(24)
                && !referenced(state, &key).await?
            {
                if !dry_run {
                    sqlx::query("INSERT INTO storage_cleanup(storage_key,not_before) VALUES($1,now()) ON CONFLICT DO NOTHING").bind(&key).execute(&state.db).await?;
                }
                orphan_keys.push(key);
            }
        }
        if !dry_run {
            sqlx::query("UPDATE cache_policy SET orphan_cursor=$1")
                .bind(if more_objects { cursor } else { None })
                .execute(&state.db)
                .await?;
        }
    }
    let due:Vec<String>=sqlx::query_scalar("SELECT storage_key FROM storage_cleanup WHERE not_before<=now() ORDER BY not_before LIMIT 500").fetch_all(&state.db).await?;
    let mut cleaned = 0;
    let mut failures = 0;
    if !dry_run {
        for key in due {
            if referenced(state, &key).await? {
                sqlx::query("DELETE FROM storage_cleanup WHERE storage_key=$1")
                    .bind(key)
                    .execute(&state.db)
                    .await?;
                continue;
            }
            match state.storage.delete(&ObjectPath::from(key.as_str())).await {
                Ok(()) | Err(object_store::Error::NotFound { .. }) => {
                    sqlx::query("DELETE FROM storage_cleanup WHERE storage_key=$1")
                        .bind(key)
                        .execute(&state.db)
                        .await?;
                    cleaned += 1;
                }
                Err(_) => {
                    failures += 1;
                }
            }
        }
    }
    let result = json!({"dryRun":dry_run,"selected":selected,"reclaimedBytes":used-remaining,"remainingBytes":remaining,"quotaSatisfied":policy.max_bytes==0||remaining<=policy.max_bytes,"orphanCandidates":orphan_keys,"scannedObjects":scanned,"moreObjects":more_objects,"deletedObjects":cleaned,"storageCleanupFailures":failures});
    if !dry_run {
        sqlx::query("UPDATE cache_policy SET last_run=now(),last_result=$1")
            .bind(&result)
            .execute(&state.db)
            .await?;
    }
    Ok(result)
}
pub async fn run(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<Run>,
) -> ApiResult<Json<Value>> {
    admin(&state, &headers)?;
    Ok(Json(
        tokio::time::timeout(
            Duration::from_secs(60),
            execute(&state, input.dry_run, input.orphans),
        )
        .await
        .map_err(|_| {
            ApiError(
                StatusCode::GATEWAY_TIMEOUT,
                "Maintenance timed out; committed batches will be retried safely".into(),
            )
        })??,
    ))
}
pub fn worker(state: AppState) {
    tokio::spawn(async move {
        loop {
            let mut states = vec![state.clone()];
            match crate::feeds::background_states(&state).await {
                Ok(feeds) => states.extend(feeds),
                Err(e) => tracing::warn!(error=%e.1,"Feed maintenance discovery failed"),
            };
            for state in states {
                let enabled = sqlx::query_scalar::<_, bool>(
                    "SELECT enabled FROM cache_policy WHERE singleton",
                )
                .fetch_one(&state.db)
                .await
                .unwrap_or(false);
                if enabled {
                    let result =
                        tokio::time::timeout(Duration::from_secs(60), execute(&state, false, true))
                            .await;
                    if !matches!(result, Ok(Ok(_))) {
                        tracing::warn!(
                            "Cache maintenance failed or timed out; next run will retry"
                        );
                    }
                }
            }
            tokio::time::sleep(Duration::from_secs(600)).await;
        }
    });
}
