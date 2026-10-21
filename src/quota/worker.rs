use crate::{AppState, error::ApiResult};
use std::time::Duration;
async fn sample(state: &AppState) -> ApiResult<()> {
    let mut tx = state.db.begin().await?;
    sqlx::query("INSERT INTO registry_usage_history SELECT ((now() AT TIME ZONE 'UTC')::date),package_count,version_count,package_bytes,symbol_bytes,now() FROM registry_usage WHERE singleton ON CONFLICT(day) DO UPDATE SET package_count=EXCLUDED.package_count,version_count=EXCLUDED.version_count,package_bytes=EXCLUDED.package_bytes,symbol_bytes=EXCLUDED.symbol_bytes,sampled_at=EXCLUDED.sampled_at")
        .execute(&mut *tx).await?;
    sqlx::query(
        "DELETE FROM registry_usage_history WHERE day<((now() AT TIME ZONE 'UTC')::date)-365",
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}
pub fn worker(state: AppState) {
    tokio::spawn(async move {
        loop {
            let mut scopes = vec![state.clone()];
            match crate::feeds::background_states(&state).await {
                Ok(feeds) => scopes.extend(feeds),
                Err(e) => tracing::warn!(error=%e.1,"Could not enumerate usage feeds"),
            }
            for scope in scopes {
                if let Err(e) = sample(&scope).await {
                    tracing::warn!(feed=?scope.feed,error=%e.1,"Usage history sampling failed");
                }
            }
            tokio::time::sleep(Duration::from_secs(300)).await;
        }
    });
}
