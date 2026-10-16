//! Abuse protection, security headers and the storage write probe.
use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use nushelf::{
    AppState,
    config::{Config, Limits},
};
use sqlx::PgPool;
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;

const ADMIN: &str = "guards-only-admin-secret-at-least-32-characters";

async fn state(storage_path: &std::path::Path, public_url: &str, limits: Limits) -> AppState {
    let database_url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    let root = PgPool::connect(&database_url).await.unwrap();
    let name = format!("guards_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE DATABASE {name}"))
        .execute(&root)
        .await
        .unwrap();
    let mut url = url::Url::parse(&database_url).unwrap();
    url.set_path(&format!("/{name}"));
    let db = PgPool::connect(url.as_str()).await.unwrap();
    sqlx::migrate!("./migrations").run(&db).await.unwrap();
    let config = Config {
        database_url: url.to_string(),
        public_url: public_url.into(),
        bind: "127.0.0.1:0".parse().unwrap(),
        admin_key: ADMIN.into(),
        max_package_bytes: 1024 * 1024,
        database_connections: 4,
        storage_provider: "local".into(),
        storage_path: storage_path.to_str().unwrap().into(),
        metrics_bind: None,
        limits,
    };
    AppState {
        db,
        storage: nushelf::storage::build(&config).unwrap(),
        config: Arc::new(config),
        upload_slots: Arc::new(tokio::sync::Semaphore::new(1)),
        guard: Default::default(),
        feeds: Arc::default(),
        feed: None,
        read_prefix: String::new(),
    }
}
fn get(path: &str, credential: Option<&str>) -> Request<Body> {
    let mut request = Request::get(path);
    if let Some(credential) = credential {
        request = request.header(header::AUTHORIZATION, format!("Bearer {credential}"));
    }
    request.body(Body::empty()).unwrap()
}

#[tokio::test]
#[ignore = "requires Docker PostgreSQL"]
async fn repeated_bad_credentials_are_throttled_but_anonymous_reads_are_not() {
    let dir = tempfile::tempdir().unwrap();
    let app = nushelf::router(
        state(
            dir.path(),
            "http://localhost:5000",
            Limits {
                auth_failures: 3,
                ..Limits::default()
            },
        )
        .await,
    );
    for _ in 0..3 {
        let response = app
            .clone()
            .oneshot(get("/api/profiles", Some("wrong-key")))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    let blocked = app
        .clone()
        .oneshot(get("/api/profiles", Some("wrong-key")))
        .await
        .unwrap();
    assert_eq!(blocked.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(blocked.headers().contains_key(header::RETRY_AFTER));
    // Even the right key waits out the window: the client has lost its budget.
    let admin = app
        .clone()
        .oneshot(get("/api/profiles", Some(ADMIN)))
        .await
        .unwrap();
    assert_eq!(admin.status(), StatusCode::TOO_MANY_REQUESTS);
    // Requests without credentials are never counted or refused.
    for path in ["/v3/index.json", "/health/live", "/api/profiles"] {
        let status = app.clone().oneshot(get(path, None)).await.unwrap().status();
        assert_ne!(status, StatusCode::TOO_MANY_REQUESTS, "{path}");
    }
}

#[tokio::test]
#[ignore = "requires Docker PostgreSQL"]
async fn disabled_limiter_never_blocks() {
    let dir = tempfile::tempdir().unwrap();
    let app = nushelf::router(
        state(
            dir.path(),
            "http://localhost:5000",
            Limits {
                auth_failures: 0,
                ..Limits::default()
            },
        )
        .await,
    );
    for _ in 0..30 {
        let status = app
            .clone()
            .oneshot(get("/api/profiles", Some("wrong-key")))
            .await
            .unwrap()
            .status();
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
}

#[tokio::test]
#[ignore = "requires Docker PostgreSQL"]
async fn security_headers_and_https_only_hsts() {
    let dir = tempfile::tempdir().unwrap();
    let plain =
        nushelf::router(state(dir.path(), "http://localhost:5000", Limits::default()).await);
    let response = plain.oneshot(get("/health/live", None)).await.unwrap();
    let headers = response.headers();
    assert!(headers.contains_key("permissions-policy"));
    assert_eq!(headers["cross-origin-opener-policy"], "same-origin");
    assert!(!headers.contains_key(header::STRICT_TRANSPORT_SECURITY));

    let secure =
        nushelf::router(state(dir.path(), "https://nuget.example.com", Limits::default()).await);
    let response = secure.oneshot(get("/health/live", None)).await.unwrap();
    assert_eq!(
        response.headers()[header::STRICT_TRANSPORT_SECURITY],
        "max-age=31536000"
    );
}

#[tokio::test]
#[ignore = "requires Docker PostgreSQL"]
async fn readiness_requires_writable_storage() {
    let dir = tempfile::tempdir().unwrap();
    let ready =
        nushelf::router(state(dir.path(), "http://localhost:5000", Limits::default()).await);
    let response = ready.oneshot(get("/health/ready", None)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    // The probe prefix is unusable (a file occupies its place), so writes fail.
    let blocked_dir = tempfile::tempdir().unwrap();
    std::fs::write(blocked_dir.path().join(".nushelf-readiness-probe"), b"x").unwrap();
    let app = nushelf::router(
        state(
            blocked_dir.path(),
            "http://localhost:5000",
            Limits::default(),
        )
        .await,
    );
    let response = app.oneshot(get("/health/ready", None)).await.unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}
