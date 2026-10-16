use crate::{
    AppState, auth,
    error::{ApiError, ApiResult},
    upstream::sources::Source,
};
use axum::{
    Json,
    body::{Body, to_bytes},
    extract::{Path, Query, Request, State},
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use hmac::{Hmac, Mac};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::Sha256;
use sqlx::FromRow;
use std::time::Duration;
use uuid::Uuid;
fn kind(method: &str, path: &str) -> &'static str {
    if path.ends_with("/api/v2/symbolpackage") || path.ends_with("/api/v2/symbolpackage/") {
        "symbols.published"
    } else if method == "DELETE" && path.contains("/api/packages/") && path.ends_with("/symbols") {
        "symbols.deleted"
    } else if path.ends_with("/api/v2/package") || path.ends_with("/api/v2/package/") {
        "package.published"
    } else if path.contains("/api/v2/package/") {
        if method == "DELETE" {
            "package.unlisted"
        } else {
            "package.relisted"
        }
    } else if path.contains("/keys") {
        if method == "DELETE" {
            "credential.revoked"
        } else {
            "credential.created"
        }
    } else if path.contains("/upstreams") {
        "upstream.changed"
    } else if path.contains("/cache/") {
        "cache.changed"
    } else {
        "management.changed"
    }
}
pub async fn record(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let method = request.method().as_str().to_owned();
    let path = request.uri().path().to_owned();
    if state.feed.is_some()
        || ["GET", "HEAD", "OPTIONS"].contains(&method.as_str())
        || path.ends_with("/mcp")
        || (!path.starts_with("/api/") && !path.starts_with("/feeds/"))
    {
        return next.run(request).await;
    }
    let admin = auth::is_admin(&state, request.headers());
    let feed = if let Some(name) = path
        .strip_prefix("/feeds/")
        .and_then(|p| p.split('/').next())
    {
        match sqlx::query_scalar::<_, Uuid>("SELECT id FROM public.feeds WHERE name=$1")
            .bind(name)
            .fetch_optional(&state.db)
            .await
        {
            Ok(id) => id,
            Err(e) => return ApiError::internal(e).into_response(),
        }
    } else {
        None
    };
    let mut auth_state = state.clone();
    auth_state.feed = feed;
    let who = if admin {
        None
    } else {
        auth::authenticate(&auth_state, request.headers())
            .await
            .ok()
    };
    let id = Uuid::new_v4();
    let event_type = kind(&method, &path);
    if let Err(e)=sqlx::query("INSERT INTO public.audit_events(id,feed_id,actor_profile,actor_key,administrator,event_type,method,path) VALUES($1,$2,$3,$4,$5,$6,$7,$8)").bind(id).bind(feed).bind(who.as_ref().map(|w|w.profile_id)).bind(who.as_ref().map(|w|w.id)).bind(admin).bind(event_type).bind(&method).bind(&path).execute(&state.db).await{return ApiError::internal(e).into_response();}
    let response = next.run(request).await;
    let status = response.status();
    let (parts, body) = response.into_parts();
    let bytes = match to_bytes(body, 1024 * 1024).await {
        Ok(b) => b,
        Err(e) => return ApiError::internal(e).into_response(),
    };
    let value = serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null);
    // Never persist response credentials, request bodies, authorization, or query strings.
    let mut target = serde_json::Map::new();
    for name in ["id", "version", "name", "hash"] {
        if let Some(value) = value.get(name).filter(|v| v.is_string()) {
            target.insert(name.into(), value.clone());
        }
    }
    let target = Value::Object(target);
    let result=async{
        let mut tx=state.db.begin().await?;
        sqlx::query("UPDATE public.audit_events SET status=$2,target=$3,completed_at=now() WHERE id=$1").bind(id).bind(status.as_u16() as i32).bind(target).execute(&mut *tx).await?;
        if status.is_success(){let hooks:Vec<Uuid>=sqlx::query_scalar("SELECT id FROM public.webhooks WHERE enabled AND (feed_id IS NULL OR feed_id=$1) AND ('*'=ANY(events) OR $2=ANY(events))").bind(feed).bind(event_type).fetch_all(&mut *tx).await?;for hook in hooks{sqlx::query("INSERT INTO public.webhook_deliveries(id,webhook_id,event_id) VALUES($1,$2,$3) ON CONFLICT DO NOTHING").bind(Uuid::new_v4()).bind(hook).bind(id).execute(&mut *tx).await?;}}
        tx.commit().await?;Ok::<_,sqlx::Error>(())
    }.await;
    if let Err(e) = result {
        tracing::error!(%id,error=%e,"Action completed; audit event remains pending reconciliation");
    }
    let mut response = Response::from_parts(parts, Body::from(bytes));
    response
        .headers_mut()
        .insert("x-nushelf-event-id", id.to_string().parse().unwrap());
    response
}
#[derive(Deserialize)]
pub struct Page {
    take: Option<i64>,
    #[serde(default)]
    skip: i64,
}
fn pagination(page: &Page) -> ApiResult<i64> {
    let take = page.take.unwrap_or(100);
    if !(1..=200).contains(&take) || !(0..=100000).contains(&page.skip) {
        return Err(ApiError::bad("Invalid event pagination"));
    }
    Ok(take)
}
pub async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(page): Query<Page>,
) -> ApiResult<Json<Value>> {
    crate::upstream::sources::admin(&state, &headers)?;
    let take = pagination(&page)?;
    let rows:Vec<Value>=sqlx::query_scalar("SELECT jsonb_build_object('id',e.id,'feedId',e.feed_id,'actor',CASE WHEN administrator THEN 'Workspace administrator' ELSE COALESCE(p.name,'Unauthenticated') END,'keyId',actor_key,'type',event_type,'method',method,'path',path,'status',status,'target',target,'createdAt',e.created_at,'completedAt',completed_at) FROM public.audit_events e LEFT JOIN public.profiles p ON p.id=e.actor_profile WHERE $3::uuid IS NULL OR e.feed_id=$3 ORDER BY e.created_at DESC,e.id LIMIT $1 OFFSET $2").bind(take).bind(page.skip).bind(state.feed).fetch_all(&state.db).await?;
    Ok(Json(json!(rows)))
}
#[derive(FromRow)]
struct Hook {
    id: Uuid,
    name: String,
    url: String,
    secret_env: String,
    events: Vec<String>,
    feed_id: Option<Uuid>,
    enabled: bool,
    allow_http: bool,
    allow_private_network: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HookInput {
    name: String,
    url: String,
    secret_env: String,
    events: Vec<String>,
    feed_id: Option<Uuid>,
    #[serde(default = "enabled")]
    enabled: bool,
    #[serde(default)]
    allow_http: bool,
    #[serde(default)]
    allow_private_network: bool,
}
fn enabled() -> bool {
    true
}
fn validate(input: &HookInput) -> ApiResult<()> {
    let url = url::Url::parse(&input.url).map_err(|_| ApiError::bad("Invalid webhook URL"))?;
    if input.name.trim().is_empty()
        || input.name.len() > 80
        || !matches!(url.scheme(), "https" | "http")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.query().is_some()
        || (url.scheme() == "http" && !input.allow_http)
    {
        return Err(ApiError::bad(
            "Webhook needs a name and safe HTTPS URL (HTTP requires explicit permission)",
        ));
    }
    if !input.secret_env.starts_with("WEBHOOK_")
        || !input
            .secret_env
            .bytes()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_')
    {
        return Err(ApiError::bad(
            "Signing secret must reference a WEBHOOK_ environment variable",
        ));
    }
    if input.events.is_empty()
        || input.events.len() > 20
        || input.events.iter().any(|e| {
            ![
                "*",
                "package.published",
                "package.unlisted",
                "package.relisted",
                "symbols.published",
                "symbols.deleted",
                "credential.created",
                "credential.revoked",
                "upstream.changed",
                "cache.changed",
                "management.changed",
            ]
            .contains(&e.as_str())
        })
    {
        return Err(ApiError::bad("Unknown webhook event type"));
    }
    Ok(())
}
pub async fn hooks(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Json<Value>> {
    crate::upstream::sources::admin(&state, &headers)?;
    let rows: Vec<Hook> = sqlx::query_as("SELECT * FROM public.webhooks ORDER BY created_at DESC")
        .fetch_all(&state.db)
        .await?;
    Ok(Json(json!(rows.into_iter().map(|h|json!({"id":h.id,"name":h.name,"url":h.url,"secretEnv":h.secret_env,"events":h.events,"feedId":h.feed_id,"enabled":h.enabled,"allowHttp":h.allow_http,"allowPrivateNetwork":h.allow_private_network})).collect::<Vec<_>>())))
}
pub async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<HookInput>,
) -> ApiResult<Json<Value>> {
    crate::upstream::sources::admin(&state, &headers)?;
    validate(&input)?;
    let mut tx = state.db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(314159270)")
        .execute(&mut *tx)
        .await?;
    if sqlx::query_scalar::<_, i64>("SELECT count(*) FROM public.webhooks")
        .fetch_one(&mut *tx)
        .await?
        >= 32
    {
        return Err(ApiError::bad("At most 32 webhook endpoints supported"));
    }
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO public.webhooks(id,name,url,secret_env,events,feed_id,enabled,allow_http,allow_private_network) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)").bind(id).bind(input.name).bind(input.url).bind(input.secret_env).bind(input.events).bind(input.feed_id).bind(input.enabled).bind(input.allow_http).bind(input.allow_private_network).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(json!({"id":id})))
}
pub async fn disable(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> ApiResult<StatusCode> {
    crate::upstream::sources::admin(&state, &headers)?;
    sqlx::query("UPDATE public.webhooks SET enabled=false WHERE id=$1")
        .bind(id)
        .execute(&state.db)
        .await?;
    sqlx::query("UPDATE public.webhook_deliveries SET completed_at=now(),last_error='Webhook disabled' WHERE webhook_id=$1 AND completed_at IS NULL").bind(id).execute(&state.db).await?;
    Ok(StatusCode::NO_CONTENT)
}
pub async fn deliveries(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    crate::upstream::sources::admin(&state, &headers)?;
    let rows:Vec<Value>=sqlx::query_scalar("SELECT jsonb_build_object('id',d.id,'eventId',d.event_id,'name',w.name,'attempts',d.attempts,'nextAttempt',d.next_attempt,'completedAt',d.completed_at,'status',d.last_status,'error',d.last_error) FROM public.webhook_deliveries d JOIN public.webhooks w ON w.id=d.webhook_id ORDER BY d.next_attempt DESC LIMIT 200").fetch_all(&state.db).await?;
    Ok(Json(json!(rows)))
}
pub fn signature(secret: &str, timestamp: i64, body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
        .expect("HMAC accepts arbitrary key sizes");
    mac.update(format!("{timestamp}.").as_bytes());
    mac.update(body);
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}
async fn send(hook: &Hook, delivery: Uuid, payload: &Value) -> ApiResult<u16> {
    let secret = std::env::var(&hook.secret_env)
        .ok()
        .filter(|s| s.len() >= 32)
        .ok_or_else(|| {
            ApiError::bad("Webhook signing secret is missing or shorter than 32 characters")
        })?;
    let source = Source {
        id: hook.id,
        name: hook.name.clone(),
        url: hook.url.clone(),
        priority: 0,
        enabled: true,
        patterns: vec![],
        excludes: vec![],
        private: false,
        auth_type: "none".into(),
        username: None,
        credential_env: None,
        allow_http: hook.allow_http,
        allow_private_network: hook.allow_private_network,
        timeout_seconds: 10,
        metadata_ttl_seconds: 1,
        stale_on_error: false,
        service_index: None,
        index_checked_at: None,
        allowed_profiles: vec![],
    };
    let url = url::Url::parse(&hook.url).map_err(ApiError::internal)?;
    let client = crate::upstream::http::client(&source, &url).await?;
    let body = serde_json::to_vec(payload).map_err(ApiError::internal)?;
    let timestamp = chrono::Utc::now().timestamp();
    let response = client
        .post(url)
        .header("content-type", "application/json")
        .header("x-nushelf-delivery", delivery.to_string())
        .header("x-nushelf-timestamp", timestamp.to_string())
        .header("x-nushelf-signature", signature(&secret, timestamp, &body))
        .header(
            "x-nushelf-event",
            payload["type"].as_str().unwrap_or("management.changed"),
        )
        .body(body)
        .send()
        .await
        .map_err(|_| ApiError::bad("Webhook connection failed"))?;
    Ok(response.status().as_u16())
}
pub async fn dispatch(state: &AppState) -> ApiResult<usize> {
    let mut count = 0;
    for _ in 0..20 {
        let mut tx = state.db.begin().await?;
        let row:Option<(Uuid,Uuid,Uuid,i32)>=sqlx::query_as("SELECT d.id,d.webhook_id,d.event_id,d.attempts FROM public.webhook_deliveries d JOIN public.webhooks w ON w.id=d.webhook_id WHERE d.completed_at IS NULL AND d.next_attempt<=now() AND w.enabled ORDER BY d.next_attempt FOR UPDATE OF d SKIP LOCKED LIMIT 1").fetch_optional(&mut *tx).await?;
        let Some((id, hook_id, event_id, attempts)) = row else {
            break;
        };
        let hook: Hook = sqlx::query_as("SELECT * FROM public.webhooks WHERE id=$1")
            .bind(hook_id)
            .fetch_one(&mut *tx)
            .await?;
        let payload:Value=sqlx::query_scalar("SELECT jsonb_build_object('id',id,'type',event_type,'feedId',feed_id,'actorProfileId',actor_profile,'method',method,'path',path,'status',status,'target',target,'createdAt',created_at) FROM public.audit_events WHERE id=$1").bind(event_id).fetch_one(&mut *tx).await?;
        let result = tokio::time::timeout(Duration::from_secs(12), send(&hook, id, &payload))
            .await
            .unwrap_or_else(|_| Err(ApiError::bad("Webhook delivery timed out")));
        let status = result.as_ref().ok().copied();
        let done = status.is_some_and(|s| (200..300).contains(&s)) || attempts >= 5;
        let error = match &result {
            Err(e) => Some(e.1.clone()),
            Ok(s) if !(200..300).contains(s) => Some(format!("HTTP {s}")),
            _ => None,
        };
        sqlx::query("UPDATE public.webhook_deliveries SET attempts=attempts+1,last_status=$2,last_error=$3,next_attempt=now()+($4::bigint*interval '1 second'),completed_at=CASE WHEN $5 THEN now() ELSE NULL END WHERE id=$1").bind(id).bind(status.map(|s|s as i32)).bind(error).bind(30_i64*(1_i64<<attempts.min(5))).bind(done).execute(&mut *tx).await?;
        tx.commit().await?;
        count += 1;
    }
    Ok(count)
}
pub async fn deliver_now(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    crate::upstream::sources::admin(&state, &headers)?;
    Ok(Json(json!({"processed":dispatch(&state).await?})))
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
