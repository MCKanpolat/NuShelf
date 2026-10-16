use super::{archive, discovery, repository};
use crate::{
    AppState,
    error::{ApiError, ApiResult},
};
use serde_json::json;
use uuid::Uuid;
#[derive(Debug, PartialEq)]
pub enum Progress {
    Busy,
    Idle,
    Advanced,
}
/// One durable work unit. The transaction lock coordinates replicas and is
/// automatically released even if the future is cancelled during network I/O.
pub async fn step(state: &AppState, id: Uuid) -> ApiResult<Progress> {
    let mut tx = state.db.begin().await?;
    sqlx::query("SET LOCAL idle_in_transaction_session_timeout='180s'")
        .execute(&mut *tx)
        .await?;
    let Some(mut mirror) = repository::lock(&mut tx, id).await? else {
        return Ok(Progress::Busy);
    };
    let source = repository::source(&mut tx, id).await?;
    if !source.enabled {
        return Ok(Progress::Idle);
    }
    let due = repository::due(&mut tx, id).await?;
    if !(mirror.requested || mirror.enabled && due) {
        return Ok(Progress::Idle);
    }
    if mirror.source_url.as_deref() != Some(source.url.as_str()) {
        mirror.cursor = json!({});
        mirror.status = "idle".into();
        mirror.discovery_complete = false;
        repository::reset(&mut tx, id, &source.url).await?;
    }
    if mirror.status != "running" {
        let generation = Uuid::new_v4();
        mirror.generation = Some(generation);
        mirror.discovery_complete = false;
        if mirror.cursor["kind"] == "search" {
            mirror.cursor = json!({});
        }
        repository::start(&mut tx, id, generation, &mirror.cursor).await?;
    }
    let generation = mirror
        .generation
        .ok_or_else(|| ApiError::internal("Mirror has no generation"))?;
    if let Some(job) = repository::pending(&mut tx, id).await? {
        match tokio::time::timeout(
            std::time::Duration::from_secs(120),
            archive::process(state, &source, &job),
        )
        .await
        .unwrap_or_else(|_| {
            Err(ApiError(
                axum::http::StatusCode::GATEWAY_TIMEOUT,
                "Mirror package work timed out".into(),
            ))
        }) {
            Ok(outcome) => {
                repository::enqueue(&mut tx, id, generation, &outcome.discovered).await?;
                repository::finish(&mut tx, id, &job, outcome.skipped).await?;
            }
            Err(error) => {
                repository::fail_job(&mut tx, id, &job, &error.1).await?;
            }
        }
    } else if !mirror.discovery_complete {
        match tokio::time::timeout(
            std::time::Duration::from_secs(120),
            discovery::page(state, &source, &mirror.cursor),
        )
        .await
        .unwrap_or_else(|_| {
            Err(ApiError(
                axum::http::StatusCode::GATEWAY_TIMEOUT,
                "Mirror discovery timed out".into(),
            ))
        }) {
            Ok(page) => {
                repository::enqueue(&mut tx, id, generation, &page.jobs).await?;
                repository::checkpoint(&mut tx, id, &page.cursor, page.complete).await?;
            }
            Err(error) => {
                repository::fail_scan(&mut tx, id, &error.1).await?;
            }
        }
    } else {
        if !repository::settle(&mut tx, id).await? {
            tx.commit().await?;
            return Ok(Progress::Idle);
        }
    }
    tx.commit().await?;
    Ok(Progress::Advanced)
}
