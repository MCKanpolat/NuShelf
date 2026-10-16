//! Route tables, grouped by audience. Each group is a plain `Router` so it can
//! be wrapped with the layers that suit it (timeouts, body limits).
use crate::{
    AppState, assets, audit, auth, deprecation, events, feeds, maintenance, mcp, registry, symbols,
    trusted, upstream,
};
use axum::{
    Router,
    extract::DefaultBodyLimit,
    routing::{delete, get, post, put},
};

/// Gallery pages, embedded assets and health probes.
pub fn web() -> Router<AppState> {
    Router::new()
        .route("/", get(crate::home))
        .route("/packages", get(crate::home))
        .route("/packages/{id}", get(crate::home))
        .route("/packages/{id}/{version}", get(crate::home))
        .route("/manage", get(crate::home))
        .route("/publish", get(crate::home))
        .route("/docs", get(crate::home))
        .route("/assets/app.css", get(crate::css))
        .route("/assets/app.js", get(crate::js))
        .route("/health/live", get(crate::live))
        .route("/health/ready", get(crate::ready))
}

/// Read side of the NuGet V3 protocol, including vulnerability data.
pub fn nuget_v3() -> Router<AppState> {
    Router::new()
        .route("/v3/index.json", get(registry::service_index))
        .route("/v3/search", get(upstream::search))
        .route("/v3/autocomplete", get(upstream::autocomplete))
        .route("/v3/flatcontainer/{id}/index.json", get(upstream::versions))
        .route(
            "/v3/flatcontainer/{id}/{version}/{file}",
            get(upstream::download),
        )
        .route(
            "/v3/registration/{id}/index.json",
            get(upstream::registration),
        )
        .route(
            "/v3/registration/{id}/{file}",
            get(upstream::registration_leaf),
        )
        .route("/v3/vulnerabilities/index.json", get(audit::index))
        .route("/v3/vulnerabilities/{file}", get(audit::page))
        .route("/symbols/{file}/{key}/{file_again}", get(symbols::download))
}

/// Package push. Kept apart because it needs its own body limit and timeout.
pub fn push() -> Router<AppState> {
    Router::new()
        .route("/api/v2/package", put(registry::push))
        .route("/api/v2/package/", put(registry::push))
        .route("/api/v2/symbolpackage", put(symbols::push))
        .route("/api/v2/symbolpackage/", put(symbols::push))
}

/// Package lifecycle, package metadata and statistics.
pub fn packages() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v2/package/{id}/{version}",
            delete(registry::unlist).post(registry::relist),
        )
        .route(
            "/api/v2/package/{id}/{version}/",
            delete(registry::unlist).post(registry::relist),
        )
        .route(
            "/api/packages/{id}/{version}/symbols",
            delete(symbols::delete),
        )
        .route("/api/stats", get(upstream::stats))
        .route("/api/packages/{id}", get(upstream::details))
        .route("/api/packages/{id}/{version}/assets", get(assets::details))
        .route("/api/packages/{id}/{version}/icon", get(assets::icon))
        .route(
            "/api/packages/{id}/{version}/deprecation",
            put(deprecation::put).delete(deprecation::clear),
        )
}

/// Profiles, API keys and Trusted Publishing.
pub fn identity() -> Router<AppState> {
    Router::new()
        .route("/api/me", get(auth::me))
        .route(
            "/api/profiles",
            get(auth::profiles).post(auth::create_profile),
        )
        .route(
            "/api/profiles/{id}",
            axum::routing::patch(auth::rename_profile),
        )
        .route(
            "/api/profiles/{id}/keys",
            get(auth::keys).post(auth::create_key),
        )
        .route("/api/profiles/{id}/keys/{key}", delete(auth::revoke_key))
        .route("/api/profiles/{id}/packages", get(registry::owned_packages))
        .route(
            "/api/trusted-publishing",
            get(trusted::list).post(trusted::create),
        )
        .route("/api/trusted-publishing/exchange", post(trusted::exchange))
        .route("/api/trusted-publishing/{id}", delete(trusted::disable))
}

/// Upstream sources, cache policy, vulnerability mirror and named feeds.
pub fn operations() -> Router<AppState> {
    Router::new()
        .route(
            "/api/upstreams",
            get(upstream::sources::list).post(upstream::sources::create),
        )
        .route("/api/upstreams/test", post(upstream::sources::test_input))
        .route("/api/upstreams/cache", get(upstream::sources::cache_list))
        .route(
            "/api/upstreams/{id}",
            put(upstream::sources::update).delete(upstream::sources::delete),
        )
        .route(
            "/api/upstreams/{id}/test",
            post(upstream::sources::test_saved),
        )
        .route(
            "/api/upstreams/{id}/cache",
            delete(upstream::sources::purge),
        )
        .route(
            "/api/cache/policy",
            get(maintenance::policy).patch(maintenance::settings),
        )
        .route("/api/cache/maintenance", post(maintenance::run))
        .route(
            "/api/cache/{id}/{version}/protection",
            put(maintenance::protect),
        )
        .route("/api/audit", get(audit::status).patch(audit::settings))
        .route("/api/audit/sync", post(audit::sync))
        .route(
            "/api/audit/import",
            post(audit::import).layer(DefaultBodyLimit::max(16 * 1024 * 1024)),
        )
        .route("/api/audit/packages/{id}", get(audit::package))
        .route("/api/feeds", get(feeds::list).post(feeds::create))
        .route("/api/feeds/{id}", axum::routing::patch(feeds::update))
        .route(
            "/api/feeds/{id}/members/{profile}",
            put(feeds::member).delete(feeds::remove_member),
        )
}

/// Audit log, webhooks and the MCP endpoint.
pub fn integrations() -> Router<AppState> {
    Router::new()
        .route("/api/events", get(events::list))
        .route("/api/webhooks", get(events::hooks).post(events::create))
        .route("/api/webhooks/deliveries", get(events::deliveries))
        .route("/api/webhooks/dispatch", post(events::deliver_now))
        .route("/api/webhooks/{id}", delete(events::disable))
        .route(
            "/mcp",
            post(mcp::endpoint)
                .get(mcp::no_stream)
                .delete(mcp::no_stream),
        )
}

/// Named-feed dispatch. The nested router applies its own timeouts, so this
/// route is left outside the general request timeout.
pub fn feed_dispatch() -> Router<AppState> {
    Router::new().route("/feeds/{name}/{*path}", axum::routing::any(feeds::endpoint))
}
