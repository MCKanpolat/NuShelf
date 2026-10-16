//! Administrator commands. Lock order is always receiver, then delivery.
use super::{
    model::{Hook, HookInput, UpdateInput, validate},
    worker::dispatch,
};
use crate::{
    AppState,
    error::{ApiError, ApiResult},
    upstream::sources::admin,
};
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use serde_json::{Value, json};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

pub(super) fn conflict(message: &str) -> ApiError {
    ApiError(StatusCode::CONFLICT, message.into())
}
pub(super) async fn lock_hook(tx: &mut Transaction<'_, Postgres>, id: Uuid) -> ApiResult<Hook> {
    sqlx::query_as("SELECT * FROM public.webhooks WHERE id=$1 FOR UPDATE")
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(ApiError::not_found)
}
async fn check_feed(tx: &mut Transaction<'_, Postgres>, id: Option<Uuid>) -> ApiResult<()> {
    if let Some(id) = id {
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM public.feeds WHERE id=$1)")
                .bind(id)
                .fetch_one(&mut **tx)
                .await?;
        if !exists {
            return Err(ApiError::bad("Unknown webhook feed"));
        }
    }
    Ok(())
}
pub(super) async fn cancel(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
    reason: &str,
) -> ApiResult<()> {
    sqlx::query("UPDATE public.webhook_deliveries SET completed_at=now(),last_error=$2 WHERE webhook_id=$1 AND completed_at IS NULL")
        .bind(id).bind(reason).execute(&mut **tx).await?;
    Ok(())
}
pub async fn hooks(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    admin(&state, &headers)?;
    let rows: Vec<Value> = sqlx::query_scalar("SELECT jsonb_build_object('id',id,'name',name,'url',url,'secretEnv',secret_env,'events',events,'feedId',feed_id,'enabled',enabled,'allowHttp',allow_http,'allowPrivateNetwork',allow_private_network,'revision',revision) FROM public.webhooks ORDER BY created_at DESC")
        .fetch_all(&state.db).await?;
    Ok(Json(json!(rows)))
}
pub async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<HookInput>,
) -> ApiResult<Json<Value>> {
    admin(&state, &headers)?;
    validate(&input)?;
    let mut tx = state.db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(314159270)")
        .execute(&mut *tx)
        .await?;
    check_feed(&mut tx, input.feed_id).await?;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM public.webhooks")
        .fetch_one(&mut *tx)
        .await?;
    if count >= 32 {
        return Err(ApiError::bad("At most 32 webhook endpoints supported"));
    }
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO public.webhooks(id,name,url,secret_env,events,feed_id,enabled,allow_http,allow_private_network) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)")
        .bind(id).bind(input.name.trim()).bind(input.url).bind(input.secret_env).bind(input.events).bind(input.feed_id).bind(input.enabled).bind(input.allow_http).bind(input.allow_private_network).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(json!({"id":id,"revision":1})))
}
pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    Json(input): Json<UpdateInput>,
) -> ApiResult<Json<Value>> {
    admin(&state, &headers)?;
    validate(&input.config)?;
    let mut tx = state.db.begin().await?;
    let old = lock_hook(&mut tx, id).await?;
    if old.revision != input.revision {
        return Err(conflict("Webhook changed. Reload before saving."));
    }
    let config = input.config;
    check_feed(&mut tx, config.feed_id).await?;
    let mut old_events = old.events.clone();
    let mut new_events = config.events.clone();
    old_events.sort();
    old_events.dedup();
    new_events.sort();
    new_events.dedup();
    if !config.enabled
        || old.url != config.url
        || old.secret_env != config.secret_env
        || old_events != new_events
        || old.feed_id != config.feed_id
        || old.allow_http != config.allow_http
        || old.allow_private_network != config.allow_private_network
    {
        cancel(
            &mut tx,
            id,
            if config.enabled {
                "Webhook configuration changed"
            } else {
                "Webhook disabled"
            },
        )
        .await?;
    }
    sqlx::query("UPDATE public.webhooks SET name=$2,url=$3,secret_env=$4,events=$5,feed_id=$6,enabled=$7,allow_http=$8,allow_private_network=$9,revision=revision+1 WHERE id=$1")
        .bind(id).bind(config.name.trim()).bind(config.url).bind(config.secret_env).bind(new_events).bind(config.feed_id).bind(config.enabled).bind(config.allow_http).bind(config.allow_private_network).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(json!({"id":id,"revision":old.revision+1})))
}
pub async fn disable(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> ApiResult<StatusCode> {
    admin(&state, &headers)?;
    let mut tx = state.db.begin().await?;
    lock_hook(&mut tx, id).await?;
    sqlx::query("UPDATE public.webhooks SET enabled=false,revision=revision+1 WHERE id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    cancel(&mut tx, id, "Webhook disabled").await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
pub async fn deliveries(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    admin(&state, &headers)?;
    let rows: Vec<Value> = sqlx::query_scalar(
        "SELECT jsonb_build_object('id',d.id,'eventId',d.event_id,'webhookId',w.id,'name',w.name,'attempts',d.attempts,'attemptsInCycle',d.attempts_in_cycle,'retryCount',d.retry_count,'nextAttempt',d.next_attempt,'completedAt',d.completed_at,'status',d.last_status,'error',d.last_error,'state',CASE WHEN d.completed_at IS NULL THEN 'pending' WHEN d.last_error IN ('Webhook disabled','Webhook configuration changed') THEN 'cancelled' WHEN d.last_status BETWEEN 200 AND 299 THEN 'delivered' ELSE 'failed' END,'canRetry',d.completed_at IS NOT NULL AND (d.last_status IS NULL OR d.last_status NOT BETWEEN 200 AND 299 OR d.last_error IN ('Webhook disabled','Webhook configuration changed')) AND w.enabled AND (w.feed_id IS NULL OR w.feed_id=e.feed_id) AND ('*'=ANY(w.events) OR e.event_type=ANY(w.events))) FROM public.webhook_deliveries d JOIN public.webhooks w ON w.id=d.webhook_id JOIN public.audit_events e ON e.id=d.event_id ORDER BY d.next_attempt DESC,d.id LIMIT 200")
        .fetch_all(&state.db).await?;
    Ok(Json(json!(rows)))
}
pub async fn retry(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> ApiResult<(StatusCode, Json<Value>)> {
    admin(&state, &headers)?;
    let mut tx = state.db.begin().await?;
    let hook_id: Uuid =
        sqlx::query_scalar("SELECT webhook_id FROM public.webhook_deliveries WHERE id=$1")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(ApiError::not_found)?;
    let hook = lock_hook(&mut tx, hook_id).await?;
    let (event_id, completed, status, error): (Uuid, bool, Option<i32>, Option<String>) =
        sqlx::query_as("SELECT event_id,completed_at IS NOT NULL,last_status,last_error FROM public.webhook_deliveries WHERE id=$1 FOR UPDATE")
            .bind(id).fetch_one(&mut *tx).await?;
    if !hook.enabled {
        return Err(conflict("Enable the webhook before retrying"));
    }
    if !completed {
        return Err(conflict("Delivery is already pending"));
    }
    let cancelled = matches!(
        error.as_deref(),
        Some("Webhook disabled" | "Webhook configuration changed")
    );
    if status.is_some_and(|s| (200..300).contains(&s)) && !cancelled {
        return Err(conflict("Delivery already succeeded"));
    }
    let payload = super::worker::payload(&mut tx, event_id).await?;
    if !hook.matches(&payload) {
        return Err(conflict(
            "Event no longer matches this webhook subscription",
        ));
    }
    sqlx::query("UPDATE public.webhook_deliveries SET attempts_in_cycle=0,retry_count=retry_count+1,completed_at=NULL,next_attempt=now(),last_status=NULL,last_error=NULL WHERE id=$1")
        .bind(id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok((StatusCode::ACCEPTED, Json(json!({"id":id}))))
}
pub async fn attempts(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    admin(&state, &headers)?;
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM public.webhook_deliveries WHERE id=$1)")
            .bind(id)
            .fetch_one(&state.db)
            .await?;
    if !exists {
        return Err(ApiError::not_found());
    }
    let rows: Vec<Value> = sqlx::query_scalar("SELECT jsonb_build_object('attempt',attempt_number,'cycle',cycle,'webhookRevision',webhook_revision,'receiverUrl',receiver_url,'startedAt',started_at,'completedAt',completed_at,'status',status,'error',error) FROM public.webhook_delivery_attempts WHERE delivery_id=$1 ORDER BY attempt_number")
        .bind(id).fetch_all(&state.db).await?;
    Ok(Json(json!(rows)))
}
pub async fn deliver_now(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    admin(&state, &headers)?;
    Ok(Json(json!({"processed":dispatch(&state).await?})))
}
