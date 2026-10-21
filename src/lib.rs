pub mod assets;
pub mod audit;
pub mod auth;
pub mod backup;
pub mod catalog;
pub mod comparison;
pub mod config;
pub mod dependents;
pub mod deprecation;
pub mod diagnostics;
pub mod error;
pub mod events;
pub mod feeds;
pub mod guard;
pub mod integrity;
pub mod maintenance;
pub mod mcp;
pub mod odata;
pub mod ownership;
pub mod package;
pub mod quota;
pub mod registry;
pub mod routes;
pub mod storage;
pub mod symbols;
pub mod telemetry;
pub mod trusted;
pub mod upstream;
pub mod version;
pub mod webhooks;

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State},
    http::{HeaderValue, StatusCode, header},
    response::{Html, IntoResponse},
};
use config::Config;
use futures_util::StreamExt;
use object_store::ObjectStore;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::{sync::Arc, time::Duration};
use tower_http::{
    compression::CompressionLayer,
    set_header::SetResponseHeaderLayer,
    timeout::{RequestBodyTimeoutLayer, ResponseBodyTimeoutLayer, TimeoutLayer},
    trace::TraceLayer,
};

#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub storage: Arc<dyn ObjectStore>,
    pub config: Arc<Config>,
    pub upload_slots: Arc<tokio::sync::Semaphore>,
    pub guard: Arc<guard::Guard>,
    pub feeds: Arc<feeds::FeedPools>,
    pub feed: Option<uuid::Uuid>,
    pub read_prefix: String,
}

/// The complete application: the scoped router plus the guards that must see
/// the real client connection, so they run once at the edge and never again
/// for requests re-dispatched into a named feed.
pub fn router(state: AppState) -> Router {
    let limits = state.config.limits.clone();
    scoped_router(state.clone())
        .layer(RequestBodyTimeoutLayer::new(limits.body_idle_timeout))
        .layer(ResponseBodyTimeoutLayer::new(limits.body_idle_timeout))
        .layer(axum::middleware::from_fn_with_state(
            state,
            guard::limit_auth_failures,
        ))
}

/// Routes and per-request layers. Named feeds dispatch into this directly
/// with a feed-scoped state.
pub fn scoped_router(state: AppState) -> Router {
    let limits = &state.config.limits;
    let timed = Router::new()
        .merge(routes::web())
        .merge(routes::nuget_v3())
        .merge(routes::nuget_v2())
        .merge(routes::packages())
        .merge(routes::identity())
        .merge(routes::operations())
        .merge(routes::integrations())
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            limits.request_timeout,
        ));
    let upload = routes::push()
        .layer(DefaultBodyLimit::max(
            state.config.max_package_bytes + 1024 * 1024,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            limit_uploads,
        ))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            limits.upload_timeout,
        ));
    let hsts = state
        .config
        .public_url
        .starts_with("https://")
        .then(|| HeaderValue::from_static("max-age=31536000"));
    Router::new()
        .merge(timed)
        .merge(upload)
        .merge(routes::feed_dispatch())
        .route_layer(axum::middleware::from_fn(telemetry::track))
        .layer(DefaultBodyLimit::max(64*1024))
        .layer(axum::middleware::from_fn_with_state(state.clone(),events::record))
        .layer(CompressionLayer::new())
        .layer(TraceLayer::new_for_http())
        .layer(SetResponseHeaderLayer::if_not_present(header::X_CONTENT_TYPE_OPTIONS,HeaderValue::from_static("nosniff")))
        .layer(SetResponseHeaderLayer::if_not_present(header::REFERRER_POLICY,HeaderValue::from_static("same-origin")))
        .layer(SetResponseHeaderLayer::if_not_present(header::CACHE_CONTROL,HeaderValue::from_static("no-store")))
        .layer(SetResponseHeaderLayer::if_not_present(header::HeaderName::from_static("permissions-policy"),HeaderValue::from_static("camera=(), microphone=(), geolocation=(), payment=()")))
        .layer(SetResponseHeaderLayer::if_not_present(header::HeaderName::from_static("cross-origin-opener-policy"),HeaderValue::from_static("same-origin")))
        .layer(SetResponseHeaderLayer::if_not_present(header::STRICT_TRANSPORT_SECURITY,hsts))
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
pub(crate) async fn live() -> StatusCode {
    StatusCode::OK
}
pub(crate) async fn home() -> Html<&'static str> {
    Html(include_str!("../web/index.html"))
}
pub(crate) async fn css() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/css; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=300"),
        ],
        include_str!("../web/app.css"),
    )
}
pub(crate) async fn js() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=300"),
        ],
        include_str!("../web/app.js"),
    )
}
pub(crate) async fn upstream_js() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=300"),
        ],
        include_str!("../web/upstream-discovery.js"),
    )
}
pub(crate) async fn webhooks_js() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=300"),
        ],
        include_str!("../web/webhooks.js"),
    )
}
pub(crate) async fn ownership_js() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        include_str!("../web/ownership.js"),
    )
}
pub(crate) async fn operations_js() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        include_str!("../web/registry-operations.js"),
    )
}
const READY_TIMEOUT: Duration = Duration::from_secs(3);
const WRITE_PROBE_TTL: Duration = Duration::from_secs(30);
const STORAGE_PROBE_PREFIX: &str = ".nushelf-readiness-probe";

pub(crate) async fn ready(State(state): State<AppState>) -> (StatusCode, Json<Value>) {
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
    storage_listable(state).await && storage_writable(state).await
}
async fn storage_listable(state: &AppState) -> bool {
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
/// Writes and removes a small object, which catches read-only buckets and
/// credentials without write permission that listing cannot reveal. Success is
/// cached briefly so frequent probes do not become storage traffic.
async fn storage_writable(state: &AppState) -> bool {
    let fresh = |at: &Option<std::time::Instant>| at.is_some_and(|t| t.elapsed() < WRITE_PROBE_TTL);
    if fresh(
        &state
            .guard
            .write_probe
            .lock()
            .unwrap_or_else(|e| e.into_inner()),
    ) {
        return true;
    }
    let path = object_store::path::Path::from(format!(
        "{STORAGE_PROBE_PREFIX}/{}",
        uuid::Uuid::new_v4().simple()
    ));
    let probe = async {
        state
            .storage
            .put(&path, object_store::PutPayload::from_static(b"ok"))
            .await?;
        if let Err(e) = state.storage.delete(&path).await {
            tracing::warn!(error = %e, "readiness: could not remove the write probe object");
        }
        Ok::<_, object_store::Error>(())
    };
    match tokio::time::timeout(READY_TIMEOUT, probe).await {
        Ok(Ok(())) => {
            *state
                .guard
                .write_probe
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = Some(std::time::Instant::now());
            true
        }
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "readiness: storage write check failed");
            false
        }
        Err(_) => {
            tracing::warn!("readiness: storage write check timed out");
            false
        }
    }
}

pub(crate) async fn diagnostics_js() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        include_str!("../web/diagnostics.js"),
    )
}

pub(crate) async fn comparison_js() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        include_str!("../web/comparison.js"),
    )
}
