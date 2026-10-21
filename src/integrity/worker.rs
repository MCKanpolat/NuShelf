use super::{
    repository,
    verify::{Item, Outcome, verify},
};
use crate::{AppState, error::ApiResult};
use std::time::Duration;
use uuid::Uuid;

async fn process(state: &AppState) -> ApiResult<()> {
    for _ in 0..20 {
        let mut tx = state.db.begin().await?;
        let run: Option<Uuid>=sqlx::query_scalar("SELECT id FROM integrity_runs WHERE status IN ('queued','running') ORDER BY created_at FOR UPDATE SKIP LOCKED LIMIT 1").fetch_optional(&mut *tx).await?;
        let Some(run) = run else { return Ok(()) };
        sqlx::query("UPDATE integrity_runs SET status='running',started_at=COALESCE(started_at,now()) WHERE id=$1").bind(run).execute(&mut *tx).await?;
        let item: Option<Item>=sqlx::query_as("SELECT * FROM integrity_items WHERE run_id=$1 AND status='pending' ORDER BY storage_key FOR UPDATE LIMIT 1").bind(run).fetch_optional(&mut *tx).await?;
        let Some(item) = item else {
            sqlx::query("UPDATE integrity_runs SET status='completed',completed_at=clock_timestamp() WHERE id=$1").bind(run).execute(&mut *tx).await?;
            repository::retain(&mut tx).await?;
            tx.commit().await?;
            return Ok(());
        };
        // Keep the immutable reference alive while reading, so eviction/deletion
        // cannot turn a legitimate concurrent removal into a false missing report.
        let query = match item.kind.as_str() {
            "package" => {
                "SELECT storage_key FROM package_versions WHERE storage_key=$1 AND sha512=$2 AND size=$3 FOR KEY SHARE"
            }
            "cache" => {
                "SELECT storage_key FROM upstream_cache WHERE storage_key=$1 AND sha512=$2 AND size=$3 FOR KEY SHARE"
            }
            _ => {
                "SELECT storage_key FROM symbol_files WHERE storage_key=$1 AND sha256=$2 AND size=$3 FOR KEY SHARE"
            }
        };
        let reference: Option<String> = sqlx::query_scalar(query)
            .bind(&item.storage_key)
            .bind(&item.expected_digest)
            .bind(item.expected_size)
            .fetch_optional(&mut *tx)
            .await?;
        let result = if reference.is_none() {
            Outcome::error("obsolete")
        } else {
            tokio::time::timeout(Duration::from_secs(60), verify(state, &item))
                .await
                .unwrap_or_else(|_| Outcome::error("unavailable"))
        };
        sqlx::query("UPDATE integrity_items SET status=$3,actual_size=$4,actual_digest=$5,checked_at=clock_timestamp() WHERE run_id=$1 AND storage_key=$2")
            .bind(item.run_id).bind(item.storage_key).bind(result.status).bind(result.size).bind(result.digest).execute(&mut *tx).await?;
        tx.commit().await?;
    }
    Ok(())
}
pub fn worker(state: AppState) {
    tokio::spawn(async move {
        loop {
            let mut scopes = vec![state.clone()];
            match crate::feeds::background_states(&state).await {
                Ok(feeds) => scopes.extend(feeds),
                Err(e) => tracing::warn!(error=%e.1,"Could not enumerate integrity feeds"),
            }
            use futures_util::{StreamExt, stream};
            stream::iter(scopes)
                .for_each_concurrent(4, |scope| async move {
                    let result = async {
                        repository::enqueue(&scope, true).await?;
                        process(&scope).await
                    }
                    .await;
                    if let Err(e) = result {
                        tracing::warn!(feed=?scope.feed,error=%e.1,"Integrity worker failed");
                    }
                })
                .await;
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    });
}
