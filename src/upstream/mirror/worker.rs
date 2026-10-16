use crate::{AppState, error::ApiResult};
use std::time::Duration;
use uuid::Uuid;
pub async fn tick(state: &AppState) -> ApiResult<bool> {
    let ids:Vec<Uuid>=sqlx::query_scalar("SELECT m.source_id FROM upstream_mirrors m JOIN upstream_sources s ON s.id=m.source_id WHERE s.enabled AND (m.requested OR (m.enabled AND m.next_run<=now())) ORDER BY m.next_run,m.source_id").fetch_all(&state.db).await?;
    let mut advanced = false;
    for id in ids {
        advanced |= super::service::step(state, id).await? == super::service::Progress::Advanced;
    }
    Ok(advanced)
}
pub fn worker(state: AppState) {
    tokio::spawn(async move {
        loop {
            let mut active = false;
            let states = match crate::feeds::background_states(&state).await {
                Ok(mut states) => {
                    states.insert(0, state.clone());
                    states
                }
                Err(error) => {
                    tracing::error!(error=%error.1,"mirror feed discovery failed");
                    vec![state.clone()]
                }
            };
            for scoped in states {
                match tick(&scoped).await {
                    Ok(progress) => active |= progress,
                    Err(error) => {
                        tracing::error!(error=%error.1,feed=?scoped.feed,"mirror worker step failed")
                    }
                }
            }
            tokio::time::sleep(if active {
                Duration::from_millis(100)
            } else {
                Duration::from_secs(5)
            })
            .await;
        }
    });
}
