//! Database locks serialize configuration changes and delivery attempts across replicas.
use super::{delivery::send, model::Hook};
use crate::{
    AppState,
    error::{ApiError, ApiResult},
};
use serde_json::Value;
use sqlx::{Postgres, Transaction};
use std::time::Duration;
use uuid::Uuid;

pub(super) async fn payload(tx: &mut Transaction<'_, Postgres>, id: Uuid) -> ApiResult<Value> {
    Ok(sqlx::query_scalar("SELECT jsonb_build_object('id',id,'type',event_type,'feedId',feed_id,'actorProfileId',actor_profile,'method',method,'path',path,'status',status,'target',target,'createdAt',created_at) FROM public.audit_events WHERE id=$1")
        .bind(id).fetch_one(&mut **tx).await?)
}
pub async fn dispatch(state: &AppState) -> ApiResult<usize> {
    let mut count = 0;
    for _ in 0..20 {
        let mut tx = state.db.begin().await?;
        // Take the receiver lock first, matching edit, disable and manual retry.
        let hook: Option<Hook> = sqlx::query_as("SELECT w.* FROM public.webhooks w WHERE w.enabled AND EXISTS(SELECT 1 FROM public.webhook_deliveries d WHERE d.webhook_id=w.id AND d.completed_at IS NULL AND d.next_attempt<=now()) ORDER BY w.created_at,w.id FOR UPDATE OF w SKIP LOCKED LIMIT 1")
            .fetch_optional(&mut *tx).await?;
        let Some(hook) = hook else { break };
        let (id, event_id, attempts, cycle_attempts, cycle): (Uuid, Uuid, i32, i32, i32) =
            sqlx::query_as("SELECT id,event_id,attempts,attempts_in_cycle,retry_count FROM public.webhook_deliveries WHERE webhook_id=$1 AND completed_at IS NULL AND next_attempt<=now() ORDER BY next_attempt,id FOR UPDATE LIMIT 1")
                .bind(hook.id).fetch_one(&mut *tx).await?;
        let body = payload(&mut tx, event_id).await?;
        if !hook.matches(&body) {
            sqlx::query("UPDATE public.webhook_deliveries SET completed_at=now(),last_error='Webhook configuration changed' WHERE id=$1")
                .bind(id).execute(&mut *tx).await?;
            tx.commit().await?;
            continue;
        }
        let started = chrono::Utc::now();
        let result = tokio::time::timeout(Duration::from_secs(12), send(&hook, id, &body))
            .await
            .unwrap_or_else(|_| Err(ApiError::bad("Webhook delivery timed out")));
        let status = result.as_ref().ok().map(|s| i32::from(*s));
        let done = status.is_some_and(|s| (200..300).contains(&s)) || cycle_attempts >= 5;
        let error = match &result {
            Err(e) => Some(e.1.clone()),
            Ok(s) if !(200..300).contains(s) => Some(format!("HTTP {s}")),
            _ => None,
        };
        sqlx::query("INSERT INTO public.webhook_delivery_attempts(delivery_id,attempt_number,cycle,webhook_revision,receiver_url,started_at,status,error) VALUES($1,$2,$3,$4,$5,$6,$7,$8)")
            .bind(id).bind(attempts+1).bind(cycle).bind(hook.revision).bind(hook.url).bind(started).bind(status).bind(&error).execute(&mut *tx).await?;
        sqlx::query("UPDATE public.webhook_deliveries SET attempts=attempts+1,attempts_in_cycle=attempts_in_cycle+1,last_status=$2,last_error=$3,next_attempt=now()+($4::bigint*interval '1 second'),completed_at=CASE WHEN $5 THEN now() ELSE NULL END WHERE id=$1")
            .bind(id).bind(status).bind(error).bind(30_i64*(1_i64<<cycle_attempts.min(5))).bind(done).execute(&mut *tx).await?;
        tx.commit().await?;
        count += 1;
    }
    Ok(count)
}
pub fn worker(state: AppState) {
    tokio::spawn(async move {
        loop {
            if let Err(e) = dispatch(&state).await {
                tracing::warn!(error=%e.1,"Webhook dispatcher failed");
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    });
}
