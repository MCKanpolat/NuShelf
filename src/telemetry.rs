//! Prometheus metrics, served from a separate listener so `/metrics` is never
//! exposed through the public registry port. Nothing is recorded unless
//! `METRICS_BIND_ADDRESS` is set: without an installed recorder the `metrics`
//! macros are no-ops.
use anyhow::Result;
use axum::{
    Router,
    extract::{MatchedPath, Request},
    http::header,
    middleware::Next,
    response::{IntoResponse, Response},
    routing::get,
};
use metrics_exporter_prometheus::{Matcher, PrometheusBuilder, PrometheusHandle};
use sqlx::PgPool;
use std::time::{Duration, Instant};

const REQUESTS: &str = "nushelf_http_requests_total";
const DURATION: &str = "nushelf_http_request_duration_seconds";
const POOL: &str = "nushelf_db_pool_connections";
const BUILD: &str = "nushelf_build_info";
const DURATION_BUCKETS: &[f64] = &[
    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0,
];

/// Records request count and latency per method, matched route pattern and
/// status. Route patterns (not raw paths) keep label cardinality bounded.
pub async fn track(request: Request, next: Next) -> Response {
    let started = Instant::now();
    let method = request.method().as_str().to_owned();
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map(|path| path.as_str().to_owned())
        .unwrap_or_else(|| "unmatched".into());
    let response = next.run(request).await;
    let labels = [
        ("method", method),
        ("route", route),
        ("status", response.status().as_u16().to_string()),
    ];
    metrics::counter!(REQUESTS, &labels).increment(1);
    metrics::histogram!(DURATION, &labels).record(started.elapsed().as_secs_f64());
    response
}

/// Installs the global recorder. Call once, before serving traffic.
pub fn install() -> Result<PrometheusHandle> {
    let handle = PrometheusBuilder::new()
        .set_buckets_for_metric(Matcher::Full(DURATION.into()), DURATION_BUCKETS)?
        .install_recorder()?;
    metrics::describe_counter!(REQUESTS, "HTTP requests by method, route and status.");
    metrics::describe_histogram!(
        DURATION,
        metrics::Unit::Seconds,
        "HTTP request latency by method, route and status."
    );
    metrics::describe_gauge!(POOL, "PostgreSQL pool connections by state.");
    metrics::describe_gauge!(BUILD, "Build information; always 1.");
    metrics::gauge!(BUILD, "version" => env!("CARGO_PKG_VERSION")).set(1.0);
    Ok(handle)
}

/// Serves `GET /metrics` on its own listener until the process exits.
pub async fn serve(
    listener: tokio::net::TcpListener,
    handle: PrometheusHandle,
    db: PgPool,
) -> Result<()> {
    let upkeep = handle.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
            upkeep.run_upkeep();
        }
    });
    let app = Router::new().route(
        "/metrics",
        get(move || {
            let handle = handle.clone();
            let db = db.clone();
            async move {
                let idle = db.num_idle() as f64;
                metrics::gauge!(POOL, "state" => "idle").set(idle);
                metrics::gauge!(POOL, "state" => "active").set(f64::from(db.size()) - idle);
                (
                    [(
                        header::CONTENT_TYPE,
                        "text/plain; version=0.0.4; charset=utf-8",
                    )],
                    handle.render(),
                )
                    .into_response()
            }
        }),
    );
    tracing::info!(address = %listener.local_addr()?, "metrics endpoint is ready");
    axum::serve(listener, app).await?;
    Ok(())
}
