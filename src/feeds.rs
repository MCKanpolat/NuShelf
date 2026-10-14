//! Named feeds use separate PostgreSQL schemas and object-store prefixes.
//! Profiles and keys remain shared; authorization runs before every scoped request.
use crate::{
    AppState, auth,
    error::{ApiError, ApiResult},
};
use axum::{
    Json,
    extract::{Path, Request, State},
    http::{StatusCode, Uri},
    response::{IntoResponse, Response},
};
use object_store::prefix::PrefixStore;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::{Executor, FromRow, PgPool, postgres::PgPoolOptions};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::Mutex;
use tower::ServiceExt;
use uuid::Uuid;

#[derive(Default)]
pub struct FeedPools(pub Mutex<HashMap<Uuid, PgPool>>);
#[derive(FromRow)]
struct Feed {
    id: Uuid,
    name: String,
    private: bool,
    enabled: bool,
}
#[derive(Deserialize)]
pub struct FeedInput {
    name: String,
    #[serde(default = "private")]
    private: bool,
}
fn private() -> bool {
    true
}
pub fn check_package(state: &AppState, id: &str) -> ApiResult<()> {
    if !id.starts_with(&state.read_prefix) {
        return Err(ApiError::forbidden("Package is outside this key's prefix"));
    }
    Ok(())
}
#[derive(Deserialize)]
pub struct MemberInput {
    pub role: String,
}
pub async fn manager(state: &AppState, headers: &axum::http::HeaderMap, id: Uuid) -> ApiResult<()> {
    if auth::is_admin(state, headers) {
        return Ok(());
    }
    let who = auth::authenticate(state, headers).await?;
    who.require("feeds:manage")?;
    let allowed: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM public.feed_members WHERE feed_id=$1 AND profile_id=$2 AND role='administrator')")
        .bind(id).bind(who.profile_id).fetch_one(&state.db).await?;
    if !allowed {
        return Err(ApiError::forbidden(
            "Feed administrator membership required",
        ));
    }
    Ok(())
}
pub async fn list(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> ApiResult<Json<Value>> {
    let admin = auth::is_admin(&state, &headers);
    let profile = if admin {
        None
    } else if headers.contains_key("authorization") || headers.contains_key("x-nuget-apikey") {
        Some(auth::authenticate(&state, &headers).await?.profile_id)
    } else {
        None
    };
    let rows: Vec<Value> = sqlx::query_scalar("SELECT jsonb_build_object('id',f.id,'name',f.name,'private',f.private,'enabled',f.enabled,'role',CASE WHEN $1 THEN 'administrator' ELSE m.role END,'members',CASE WHEN $1 OR m.role='administrator' THEN (SELECT jsonb_agg(jsonb_build_object('profileId',p.id,'name',p.name,'role',fm.role)) FROM feed_members fm JOIN profiles p ON p.id=fm.profile_id WHERE fm.feed_id=f.id) ELSE NULL END) FROM feeds f LEFT JOIN feed_members m ON m.feed_id=f.id AND m.profile_id=$2 WHERE $1 OR (f.enabled AND (NOT f.private OR m.profile_id IS NOT NULL)) ORDER BY f.name")
        .bind(admin).bind(profile).fetch_all(&state.db).await?;
    Ok(Json(json!(rows)))
}
pub async fn create(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Json(input): Json<FeedInput>,
) -> ApiResult<Json<Value>> {
    if !auth::is_admin(&state, &headers) {
        return Err(ApiError::unauthorized());
    }
    let name = input.name.trim().to_ascii_lowercase();
    if name.is_empty()
        || name.len() > 40
        || !name.as_bytes()[0].is_ascii_lowercase()
        || !name
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
    {
        return Err(ApiError::bad(
            "Feed names need 1–40 lowercase letters, digits or hyphens, starting with a letter",
        ));
    }
    let id = Uuid::new_v4();
    let mut tx = state.db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(314159265)")
        .execute(&mut *tx)
        .await?;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM feeds")
        .fetch_one(&mut *tx)
        .await?;
    if count >= 100 {
        return Err(ApiError::bad("At most 100 named feeds are supported"));
    }
    let inserted = sqlx::query(
        "INSERT INTO feeds(id,name,private) VALUES($1,$2,$3) ON CONFLICT(name) DO NOTHING",
    )
    .bind(id)
    .bind(&name)
    .bind(input.private)
    .execute(&mut *tx)
    .await?;
    if inserted.rows_affected() == 0 {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "Feed name already exists".into(),
        ));
    }
    let schema = format!("feed_{}", id.simple());
    (&mut *tx).execute(format!("CREATE SCHEMA {schema}; SET LOCAL search_path TO {schema},public; CREATE VIEW profiles AS SELECT * FROM public.profiles; CREATE VIEW api_keys AS SELECT * FROM public.api_keys;").as_str()).await?;
    let initial = include_str!("../migrations/0001_registry.sql");
    let initial = &initial[initial
        .find("CREATE TABLE packages (")
        .expect("package migration")..];
    (&mut *tx)
        .execute(
            initial
                .replace("REFERENCES profiles(id)", "REFERENCES public.profiles(id)")
                .as_str(),
        )
        .await?;
    (&mut *tx)
        .execute(
            include_str!("../migrations/0002_upstreams.sql")
                .replace("REFERENCES profiles(id)", "REFERENCES public.profiles(id)")
                .as_str(),
        )
        .await?;
    tx.commit().await?;
    pool(&state, id).await?;
    Ok(Json(
        json!({"id":id,"name":name,"private":input.private,"enabled":true}),
    ))
}
pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: axum::http::HeaderMap,
    Json(input): Json<Value>,
) -> ApiResult<Json<Value>> {
    manager(&state, &headers, id).await?;
    let private = input.get("private").and_then(Value::as_bool);
    let enabled = input.get("enabled").and_then(Value::as_bool);
    let row:Option<Value>=sqlx::query_scalar("UPDATE public.feeds SET private=COALESCE($2,private),enabled=COALESCE($3,enabled) WHERE id=$1 RETURNING jsonb_build_object('id',id,'name',name,'private',private,'enabled',enabled)").bind(id).bind(private).bind(enabled).fetch_optional(&state.db).await?;
    Ok(Json(row.ok_or_else(ApiError::not_found)?))
}
pub async fn member(
    State(state): State<AppState>,
    Path((id, profile)): Path<(Uuid, Uuid)>,
    headers: axum::http::HeaderMap,
    Json(input): Json<MemberInput>,
) -> ApiResult<StatusCode> {
    manager(&state, &headers, id).await?;
    if !["reader", "publisher", "administrator"].contains(&input.role.as_str()) {
        return Err(ApiError::bad("Unknown feed role"));
    }
    sqlx::query("INSERT INTO public.feed_members(feed_id,profile_id,role) VALUES($1,$2,$3) ON CONFLICT(feed_id,profile_id) DO UPDATE SET role=EXCLUDED.role").bind(id).bind(profile).bind(input.role).execute(&state.db).await?;
    Ok(StatusCode::NO_CONTENT)
}
pub async fn remove_member(
    State(state): State<AppState>,
    Path((id, profile)): Path<(Uuid, Uuid)>,
    headers: axum::http::HeaderMap,
) -> ApiResult<StatusCode> {
    manager(&state, &headers, id).await?;
    sqlx::query("DELETE FROM public.feed_members WHERE feed_id=$1 AND profile_id=$2")
        .bind(id)
        .bind(profile)
        .execute(&state.db)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn pool(state: &AppState, id: Uuid) -> ApiResult<PgPool> {
    let mut pools = state.feeds.0.lock().await;
    if let Some(pool) = pools.get(&id) {
        return Ok(pool.clone());
    }
    let schema = format!("feed_{}", id.simple());
    let pool=PgPoolOptions::new().max_connections(4).min_connections(0).idle_timeout(Duration::from_secs(60)).after_connect(move |conn,_|{
        let sql=format!("SET search_path TO {schema},public; SET statement_timeout='15s'; SET idle_in_transaction_session_timeout='90s'");
        Box::pin(async move {conn.execute(sql.as_str()).await?;Ok(())})
    }).connect(&state.config.database_url).await?;
    sqlx::migrate!("./feed-migrations")
        .run(&pool)
        .await
        .map_err(ApiError::internal)?;
    pools.insert(id, pool.clone());
    Ok(pool)
}
pub async fn initialize(state: &AppState) -> ApiResult<()> {
    let ids: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM public.feeds")
        .fetch_all(&state.db)
        .await?;
    for id in ids {
        pool(state, id).await?;
    }
    Ok(())
}
pub async fn endpoint(
    State(state): State<AppState>,
    Path((name, path)): Path<(String, String)>,
    mut request: Request,
) -> ApiResult<Response> {
    let feed: Feed =
        sqlx::query_as("SELECT id,name,private,enabled FROM public.feeds WHERE name=$1")
            .bind(name)
            .fetch_optional(&state.db)
            .await?
            .ok_or_else(ApiError::not_found)?;
    if !feed.enabled {
        return Err(ApiError::not_found());
    }
    let safe_read = request.method() == axum::http::Method::GET
        || request.method() == axum::http::Method::HEAD
        || path == "mcp";
    let ui = path.is_empty()
        || path == "packages"
        || path.starts_with("packages/")
        || ["manage", "publish", "docs"].contains(&path.as_str());
    let mut read_prefix = String::new();
    if !ui
        && (feed.private
            || !safe_read
            || request.headers().contains_key("authorization")
            || request.headers().contains_key("x-nuget-apikey"))
        && !auth::is_admin(&state, request.headers())
    {
        let who = auth::authenticate(&state, request.headers()).await?;
        let role: Option<String> = sqlx::query_scalar(
            "SELECT role FROM public.feed_members WHERE feed_id=$1 AND profile_id=$2",
        )
        .bind(feed.id)
        .bind(who.profile_id)
        .fetch_optional(&state.db)
        .await?;
        let role = role.ok_or_else(|| ApiError::forbidden("Feed membership required"))?;
        if safe_read {
            who.require("package:read")?;
            read_prefix = who.package_prefix.clone();
        } else if role == "reader" {
            return Err(ApiError::forbidden("Publisher feed membership required"));
        }
        if let Some(id) = path
            .strip_prefix("v3/flatcontainer/")
            .or_else(|| path.strip_prefix("v3/registration/"))
            .or_else(|| path.strip_prefix("api/packages/"))
        {
            who.package(&crate::version::package_id(
                id.split('/').next().unwrap_or_default(),
            )?)?;
        }
    }
    // Control-plane management always uses root URLs; no nested feed dispatch.
    if path.starts_with("feeds/") || path.starts_with("api/feeds") {
        return Err(ApiError::not_found());
    }
    let db = pool(&state, feed.id).await?;
    let mut scoped = state.clone();
    scoped.db = db;
    scoped.feed = Some(feed.id);
    scoped.read_prefix = read_prefix;
    let mut config = (*state.config).clone();
    config.public_url = format!("{}/feeds/{}", config.public_url, feed.name);
    scoped.config = Arc::new(config);
    scoped.storage = Arc::new(PrefixStore::new(
        state.storage.clone(),
        format!("feeds/{}", feed.id.simple()),
    ));
    let query = request
        .uri()
        .query()
        .map(|q| format!("?{q}"))
        .unwrap_or_default();
    *request.uri_mut() = format!("/{path}{query}")
        .parse::<Uri>()
        .map_err(ApiError::internal)?;
    request.extensions_mut().clear();
    let response = crate::router(scoped)
        .oneshot(request)
        .await
        .expect("infallible router");
    Ok(response.into_response())
}
