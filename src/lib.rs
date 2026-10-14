pub mod assets;
pub mod audit;
pub mod auth;
pub mod config;
pub mod deprecation;
pub mod error;
pub mod events;
pub mod feeds;
pub mod maintenance;
pub mod mcp;
pub mod package;
pub mod registry;
pub mod storage;
pub mod trusted;
pub mod upstream;
pub mod version;

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State},
    http::{HeaderValue, StatusCode, header},
    response::{Html, IntoResponse},
    routing::{get, post, put},
};
use config::Config;
use futures_util::StreamExt;
use object_store::ObjectStore;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::{sync::Arc, time::Duration};
use tower_http::{
    compression::CompressionLayer, set_header::SetResponseHeaderLayer, trace::TraceLayer,
};

#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub storage: Arc<dyn ObjectStore>,
    pub config: Arc<Config>,
    pub upload_slots: Arc<tokio::sync::Semaphore>,
    pub feeds: Arc<feeds::FeedPools>,
    pub feed: Option<uuid::Uuid>,
    pub read_prefix: String,
}

pub fn router(state: AppState) -> Router {
    let upload = Router::new()
        .route("/api/v2/package", put(registry::push))
        .route("/api/v2/package/", put(registry::push))
        .layer(DefaultBodyLimit::max(
            state.config.max_package_bytes + 1024 * 1024,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            limit_uploads,
        ));
    Router::new()
        .route("/api/trusted-publishing",get(trusted::list).post(trusted::create))
        .route("/api/trusted-publishing/exchange",post(trusted::exchange))
        .route("/api/trusted-publishing/{id}",axum::routing::delete(trusted::disable))
        .route("/api/events",get(events::list))
        .route("/api/webhooks",get(events::hooks).post(events::create))
        .route("/api/webhooks/deliveries",get(events::deliveries))
        .route("/api/webhooks/dispatch",post(events::deliver_now))
        .route("/api/webhooks/{id}",axum::routing::delete(events::disable))
        .route("/api/cache/policy",get(maintenance::policy).patch(maintenance::settings))
        .route("/api/cache/maintenance",post(maintenance::run))
        .route("/api/cache/{id}/{version}/protection",put(maintenance::protect))
        .route("/v3/vulnerabilities/index.json",get(audit::index))
        .route("/v3/vulnerabilities/{file}",get(audit::page))
        .route("/api/audit",get(audit::status).patch(audit::settings))
        .route("/api/audit/sync",post(audit::sync))
        .route("/api/audit/import",post(audit::import).layer(DefaultBodyLimit::max(16*1024*1024)))
        .route("/api/audit/packages/{id}",get(audit::package))
        .route("/feeds/{name}/{*path}",axum::routing::any(feeds::endpoint))
        .route("/api/feeds",get(feeds::list).post(feeds::create))
        .route("/api/feeds/{id}",axum::routing::patch(feeds::update))
        .route("/api/feeds/{id}/members/{profile}",put(feeds::member).delete(feeds::remove_member))
        .route("/",get(home))
        .route("/packages",get(home))
        .route("/packages/{id}",get(home))
        .route("/packages/{id}/{version}",get(home))
        .route("/manage",get(home))
        .route("/publish",get(home))
        .route("/docs",get(home))
        .route("/assets/app.css",get(css))
        .route("/assets/app.js",get(js))
        .route("/health/live",get(|| async { StatusCode::OK }))
        .route("/health/ready",get(ready))
        .route("/v3/index.json",get(registry::service_index))
        .route("/v3/search",get(upstream::search))
        .route("/v3/autocomplete",get(upstream::autocomplete))
        .route("/v3/flatcontainer/{id}/index.json",get(upstream::versions))
        .route("/v3/flatcontainer/{id}/{version}/{file}",get(upstream::download))
        .route("/v3/registration/{id}/index.json",get(upstream::registration))
        .route("/v3/registration/{id}/{file}",get(upstream::registration_leaf))
        .merge(upload)
        .route("/api/v2/package/{id}/{version}",axum::routing::delete(registry::unlist).post(registry::relist))
        .route("/api/v2/package/{id}/{version}/",axum::routing::delete(registry::unlist).post(registry::relist))
        .route("/api/stats",get(upstream::stats))
        .route("/api/packages/{id}",get(upstream::details))
        .route("/api/packages/{id}/{version}/assets",get(assets::details))
        .route("/api/packages/{id}/{version}/icon",get(assets::icon))
        .route("/api/packages/{id}/{version}/deprecation",put(deprecation::put).delete(deprecation::clear))
        .route("/api/upstreams",get(upstream::sources::list).post(upstream::sources::create))
        .route("/api/upstreams/test",post(upstream::sources::test_input))
        .route("/api/upstreams/cache",get(upstream::sources::cache_list))
        .route("/api/upstreams/{id}",axum::routing::put(upstream::sources::update).delete(upstream::sources::delete))
        .route("/api/upstreams/{id}/test",post(upstream::sources::test_saved))
        .route("/api/upstreams/{id}/cache",axum::routing::delete(upstream::sources::purge))
        .route("/api/me",get(auth::me))
        .route("/api/profiles",get(auth::profiles).post(auth::create_profile))
        .route("/api/profiles/{id}",axum::routing::patch(auth::rename_profile))
        .route("/api/profiles/{id}/keys",get(auth::keys).post(auth::create_key))
        .route("/api/profiles/{id}/keys/{key}",axum::routing::delete(auth::revoke_key))
        .route("/api/profiles/{id}/packages",get(registry::owned_packages))
        .route("/mcp",post(mcp::endpoint).get(mcp::no_stream).delete(mcp::no_stream))
        .layer(DefaultBodyLimit::max(64*1024))
        .layer(axum::middleware::from_fn_with_state(state.clone(),events::record))
        .layer(CompressionLayer::new())
        .layer(TraceLayer::new_for_http())
        .layer(SetResponseHeaderLayer::if_not_present(header::X_CONTENT_TYPE_OPTIONS,HeaderValue::from_static("nosniff")))
        .layer(SetResponseHeaderLayer::if_not_present(header::REFERRER_POLICY,HeaderValue::from_static("same-origin")))
        .layer(SetResponseHeaderLayer::if_not_present(header::CACHE_CONTROL,HeaderValue::from_static("no-store")))
        .layer(SetResponseHeaderLayer::if_not_present(header::CONTENT_SECURITY_POLICY,HeaderValue::from_static("default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'")))
        .with_state(state)
}
async fn limit_uploads(
    State(state): State<AppState>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let Ok(_permit) = state.upload_slots.clone().try_acquire_owned() else {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            "Too many concurrent uploads; retry later",
        )
            .into_response();
    };
    next.run(request).await
}
async fn home() -> Html<&'static str> {
    Html(include_str!("../web/index.html"))
}
async fn css() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/css; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=300"),
        ],
        include_str!("../web/app.css"),
    )
}
async fn js() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=300"),
        ],
        include_str!("../web/app.js"),
    )
}
const READY_TIMEOUT: Duration = Duration::from_secs(3);
const STORAGE_PROBE_PREFIX: &str = ".nushelf-readiness-probe";

async fn ready(State(state): State<AppState>) -> (StatusCode, Json<Value>) {
    let (database, storage) = tokio::join!(database_ready(&state), storage_ready(&state));
    let status = if database && storage {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    let check = |ok: bool| if ok { "ok" } else { "unavailable" };
    (
        status,
        Json(json!({"checks":{"database":check(database),"storage":check(storage)}})),
    )
}
async fn database_ready(state: &AppState) -> bool {
    match tokio::time::timeout(READY_TIMEOUT, sqlx::query("SELECT 1").execute(&state.db)).await {
        Ok(Ok(_)) => true,
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "readiness: database check failed");
            false
        }
        Err(_) => {
            tracing::warn!("readiness: database check timed out");
            false
        }
    }
}
/// Lists a prefix that never holds packages. A missing bucket or container,
/// rejected credentials or an unreachable endpoint all fail the listing, while
/// an empty result proves the store is reachable and readable.
async fn storage_ready(state: &AppState) -> bool {
    let prefix = object_store::path::Path::from(STORAGE_PROBE_PREFIX);
    let probe = async {
        let mut listing = state.storage.list(Some(&prefix));
        match listing.next().await {
            None | Some(Ok(_)) => Ok(()),
            Some(Err(e)) => Err(e),
        }
    };
    match tokio::time::timeout(READY_TIMEOUT, probe).await {
        Ok(Ok(())) => true,
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "readiness: storage check failed");
            false
        }
        Err(_) => {
            tracing::warn!("readiness: storage check timed out");
            false
        }
    }
}
