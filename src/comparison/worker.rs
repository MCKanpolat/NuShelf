use super::{analyzer, archive, reference::Reference};
use crate::{
    AppState,
    error::{ApiError, ApiResult},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use bytes::Bytes;
use futures_util::StreamExt;
use serde_json::{Value, json};
use sha2::{Digest, Sha512};
use std::time::Duration;
use uuid::Uuid;

async fn read(state: &AppState, reference: &Reference) -> ApiResult<Bytes> {
    let limit = state.config.max_package_bytes.min(256 * 1024 * 1024);
    if reference.size < 0 || reference.size as u64 > limit as u64 {
        return Err(ApiError::bad(
            "Comparison archive exceeds 256 MiB or the configured package limit",
        ));
    }
    let result = state
        .storage
        .get(&object_store::path::Path::from(
            reference.storage_key.as_str(),
        ))
        .await?;
    let mut stream = result.into_stream();
    let mut bytes = Vec::new();
    let mut hash = Sha512::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if bytes.len().saturating_add(chunk.len()) > limit {
            return Err(ApiError::bad("Comparison archive exceeds size limit"));
        }
        hash.update(&chunk);
        bytes.extend_from_slice(&chunk);
    }
    if bytes.len() as i64 != reference.size || STANDARD.encode(hash.finalize()) != reference.sha512
    {
        return Err(ApiError::bad(
            "Comparison archive failed stored size/hash verification",
        ));
    }
    Ok(Bytes::from(bytes))
}
async fn analyze(
    state: &AppState,
    inputs: Value,
    baseline: &str,
    target: &str,
) -> ApiResult<Value> {
    let refs: [Reference; 2] = serde_json::from_value(inputs).map_err(ApiError::internal)?;
    let left = read(state, &refs[0]).await?;
    let right = read(state, &refs[1]).await?;
    let a = left.clone();
    let b = right.clone();
    let mut report = tokio::task::spawn_blocking(move || archive::compare(a, b))
        .await
        .map_err(ApiError::internal)??;
    let dir = tempfile::tempdir().map_err(ApiError::internal)?;
    tokio::fs::write(dir.path().join("baseline.nupkg"), left)
        .await
        .map_err(ApiError::internal)?;
    tokio::fs::write(dir.path().join("target.nupkg"), right)
        .await
        .map_err(ApiError::internal)?;
    report["api"] = analyzer::compare(dir.path()).await?;
    let before = crate::version::NuGetVersion::parse(baseline)?;
    let after = crate::version::NuGetVersion::parse(target)?;
    let same_major = before.normalized.split('.').next() == after.normalized.split('.').next();
    let findings = report["api"]["baselineToTarget"]["findings"]
        .as_array()
        .is_some_and(|v| !v.is_empty());
    report["semverReviewRecommended"] = json!(same_major && findings);
    Ok(report)
}
async fn process(state: &AppState) -> ApiResult<()> {
    let token = Uuid::new_v4();
    let job:Option<(Uuid,Value,String,String)>=sqlx::query_as("UPDATE package_comparisons SET status='running',lease_token=$1,lease_until=now()+interval '5 minutes',attempts=attempts+1 WHERE id=(SELECT id FROM package_comparisons WHERE (status='queued' OR (status='running' AND lease_until<now())) AND attempts<3 ORDER BY created_at FOR UPDATE SKIP LOCKED LIMIT 1) RETURNING id,inputs,baseline_version,target_version").bind(token).fetch_optional(&state.db).await?;
    if let Some((id, inputs, baseline, target)) = job {
        let result = tokio::time::timeout(
            Duration::from_secs(210),
            analyze(state, inputs, &baseline, &target),
        )
        .await
        .unwrap_or_else(|_| Err(ApiError::bad("Package comparison timed out")));
        let (report, error, status) = match result {
            Ok(report) => (Some(report), None, "completed"),
            Err(error) => (None, Some(error.1), "failed"),
        };
        sqlx::query("UPDATE package_comparisons SET status=$3,report=$4,error=$5,completed_at=now(),lease_token=NULL,lease_until=NULL WHERE id=$1 AND lease_token=$2").bind(id).bind(token).bind(status).bind(report).bind(error).execute(&state.db).await?;
    }
    sqlx::query("UPDATE package_comparisons SET status='failed',error='Worker stopped repeatedly; request a new comparison',completed_at=now() WHERE status='running' AND lease_until<now() AND attempts>=3").execute(&state.db).await?;
    sqlx::query("DELETE FROM package_comparisons WHERE id IN (SELECT id FROM (SELECT id,row_number() OVER(PARTITION BY package_id ORDER BY created_at DESC) AS n FROM package_comparisons WHERE status IN ('completed','failed')) r WHERE n>30)").execute(&state.db).await?;
    Ok(())
}
pub fn worker(state: AppState) {
    tokio::spawn(async move {
        loop {
            let mut scopes = vec![state.clone()];
            match crate::feeds::background_states(&state).await {
                Ok(feeds) => scopes.extend(feeds),
                Err(e) => tracing::warn!(error=%e.1,"Cannot enumerate comparison feeds"),
            }
            // One analysis per process at a time bounds RAM, disk and child processes.
            for scope in scopes {
                if let Err(e) = process(&scope).await {
                    tracing::warn!(feed=?scope.feed,error=%e.1,"Comparison worker failed");
                }
            }
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    });
}
