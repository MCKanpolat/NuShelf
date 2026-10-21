use crate::{
    AppState, auth,
    error::{ApiError, ApiResult},
};
use axum::{
    Json,
    body::{Body, to_bytes},
    extract::{Query, Request, State},
    http::HeaderMap,
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::{Value, json};
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
    } else if path.contains("/ownership") {
        if path.ends_with("/accept") {
            "package.ownership.transferred"
        } else if path.ends_with("/cancel") {
            "package.ownership.cancelled"
        } else {
            "package.ownership.requested"
        }
    } else if path.contains("/api/applications") {
        if method == "DELETE" {
            "inventory.deleted"
        } else if path.ends_with("/releases") {
            "inventory.imported"
        } else {
            "application.created"
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
        if status.is_success(){let hooks:Vec<Uuid>=sqlx::query_scalar("SELECT id FROM public.webhooks WHERE enabled AND (feed_id IS NULL OR feed_id=$1) AND ('*'=ANY(events) OR $2=ANY(events)) ORDER BY id FOR UPDATE").bind(feed).bind(event_type).fetch_all(&mut *tx).await?;for hook in hooks{sqlx::query("INSERT INTO public.webhook_deliveries(id,webhook_id,event_id) VALUES($1,$2,$3) ON CONFLICT DO NOTHING").bind(Uuid::new_v4()).bind(hook).bind(id).execute(&mut *tx).await?;}}
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

// Compatibility for existing dispatcher consumers.
pub use crate::webhooks::{dispatch, signature, worker};
