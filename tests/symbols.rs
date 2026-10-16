//! Symbol packages and the symbol server, through the real router.
use axum::{
    Router,
    body::Body,
    http::{HeaderMap, Method, Request, StatusCode, header},
};
use bytes::Bytes;
use nushelf::{AppState, config::Config};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::{
    io::{Cursor, Write},
    sync::Arc,
};
use tower::ServiceExt;
use uuid::Uuid;

const ADMIN: &str = "symbols-only-admin-secret-at-least-32-characters";

/// Smallest portable PDB the server reads: a metadata root with one `#Pdb` stream.
fn portable_pdb(id: [u8; 20], padding: usize) -> Vec<u8> {
    let version = b"PDB v1.0\0\0\0\0";
    let mut root = Vec::new();
    root.extend(0x424A_5342u32.to_le_bytes());
    root.extend([1, 0, 1, 0]);
    root.extend(0u32.to_le_bytes());
    root.extend((version.len() as u32).to_le_bytes());
    root.extend(version);
    root.extend(0u16.to_le_bytes());
    root.extend(1u16.to_le_bytes());
    let offset = (root.len() + 16) as u32;
    root.extend(offset.to_le_bytes());
    root.extend(24u32.to_le_bytes());
    root.extend(b"#Pdb\0\0\0\0");
    root.extend(id);
    root.extend([0, 0, 0, 0]);
    root.extend(vec![0xAB; padding]);
    root
}
// Bytes 00..0f as a .NET Guid, then the portable age.
const KEY: &str = "03020100050407060809\
0a0b0c0d0e0fffffffff";
fn pdb_id() -> [u8; 20] {
    std::array::from_fn(|i| i as u8)
}
fn nupkg(id: &str, version: &str) -> Bytes {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default();
    writer.start_file("package.nuspec", options).unwrap();
    write!(writer,"<package><metadata><id>{id}</id><version>{version}</version><authors>T</authors><description>d</description></metadata></package>").unwrap();
    writer.start_file("lib/net8.0/_._", options).unwrap();
    Bytes::from(writer.finish().unwrap().into_inner())
}
fn snupkg(id: &str, version: &str, pdbs: &[(&str, Vec<u8>)]) -> Bytes {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default();
    writer.start_file(format!("{id}.nuspec"), options).unwrap();
    write!(writer,"<package><metadata><id>{id}</id><version>{version}</version><packageTypes><packageType name=\"SymbolsPackage\"/></packageTypes></metadata></package>").unwrap();
    for (name, data) in pdbs {
        writer.start_file(*name, options).unwrap();
        writer.write_all(data).unwrap();
    }
    Bytes::from(writer.finish().unwrap().into_inner())
}

async fn app() -> (Router, AppState, tempfile::TempDir) {
    let database_url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    let root = PgPool::connect(&database_url).await.unwrap();
    let name = format!("symbols_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE DATABASE {name}"))
        .execute(&root)
        .await
        .unwrap();
    let mut url = url::Url::parse(&database_url).unwrap();
    url.set_path(&format!("/{name}"));
    let db = PgPool::connect(url.as_str()).await.unwrap();
    sqlx::migrate!("./migrations").run(&db).await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let config = Config {
        database_url: url.to_string(),
        public_url: "http://registry.test".into(),
        bind: "127.0.0.1:0".parse().unwrap(),
        admin_key: ADMIN.into(),
        max_package_bytes: 4 * 1024 * 1024,
        database_connections: 8,
        storage_provider: "local".into(),
        storage_path: dir.path().to_str().unwrap().into(),
        metrics_bind: None,
        limits: nushelf::config::Limits {
            auth_failures: 0,
            ..Default::default()
        },
    };
    let state = AppState {
        db,
        storage: nushelf::storage::build(&config).unwrap(),
        config: Arc::new(config),
        upload_slots: Arc::new(tokio::sync::Semaphore::new(4)),
        guard: Default::default(),
        feeds: Arc::default(),
        feed: None,
        read_prefix: String::new(),
    };
    (nushelf::router(state.clone()), state, dir)
}
async fn call(
    app: &Router,
    method: Method,
    path: &str,
    credential: Option<&str>,
    body: Option<Bytes>,
) -> (StatusCode, HeaderMap, Bytes) {
    let mut request = Request::builder().method(method).uri(path);
    if let Some(credential) = credential {
        request = request.header(header::AUTHORIZATION, format!("Bearer {credential}"));
    }
    if body.is_some() {
        request = request.header(header::CONTENT_TYPE, "application/octet-stream");
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::from(body.unwrap_or_default())).unwrap())
        .await
        .unwrap();
    let (parts, body) = response.into_parts();
    (
        parts.status,
        parts.headers,
        axum::body::to_bytes(body, 64 * 1024 * 1024).await.unwrap(),
    )
}
async fn json_call(app: &Router, method: Method, path: &str, body: Value) -> Value {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::AUTHORIZATION, format!("Bearer {ADMIN}"))
        .header(header::CONTENT_TYPE, "application/json");
    request = request.header("accept", "application/json");
    let response = app
        .clone()
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    assert!(status.is_success(), "{path}: {status} {bytes:?}");
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}
async fn profile(app: &Router, name: &str) -> String {
    json_call(app, Method::POST, "/api/profiles", json!({"name":name})).await["id"]
        .as_str()
        .unwrap()
        .into()
}
async fn key(app: &Router, profile: &str, scopes: &[&str], prefix: &str) -> String {
    json_call(
        app,
        Method::POST,
        &format!("/api/profiles/{profile}/keys"),
        json!({"label":"k","scopes":scopes,"packagePrefix":prefix}),
    )
    .await["key"]
        .as_str()
        .unwrap()
        .into()
}
fn symbol_path(file: &str, key: &str) -> String {
    format!("/symbols/{file}/{key}/{file}")
}

#[tokio::test]
#[ignore = "requires Docker PostgreSQL"]
async fn symbols_are_pushed_validated_served_and_replaceable() {
    let (app, state, _dir) = app().await;
    let owner = profile(&app, "Owner").await;
    let other = profile(&app, "Other").await;
    let push = key(&app, &owner, &["package:push", "package:unlist"], "").await;
    let narrow = key(&app, &owner, &["package:push"], "unrelated.").await;
    let intruder = key(&app, &other, &["package:push"], "").await;
    let pdb = portable_pdb(pdb_id(), 100);
    let package = snupkg(
        "Acme.Json",
        "1.0.0",
        &[("lib/net8.0/Acme.Json.pdb", pdb.clone())],
    );

    // The service index advertises the endpoint, which is what makes `dotnet nuget push` send .snupkg files.
    let (_, _, index) = call(&app, Method::GET, "/v3/index.json", None, None).await;
    let index: Value = serde_json::from_slice(&index).unwrap();
    assert!(index["resources"].as_array().unwrap().iter().any(|r| {
        r["@type"] == "SymbolPackagePublish/4.9.0"
            && r["@id"] == "http://registry.test/api/v2/symbolpackage"
    }));

    let symbol_push = |credential: &str, body: Bytes| {
        let app = app.clone();
        let credential = credential.to_owned();
        async move {
            call(
                &app,
                Method::PUT,
                "/api/v2/symbolpackage",
                Some(&credential),
                Some(body),
            )
            .await
            .0
        }
    };
    assert_eq!(
        symbol_push(&push, package.clone()).await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &app,
            Method::PUT,
            "/api/v2/package",
            Some(&push),
            Some(nupkg("Acme.Json", "1.0.0"))
        )
        .await
        .0,
        StatusCode::CREATED
    );
    assert_eq!(
        call(
            &app,
            Method::PUT,
            "/api/v2/symbolpackage",
            None,
            Some(package.clone())
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        symbol_push(&intruder, package.clone()).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        symbol_push(&narrow, package.clone()).await,
        StatusCode::FORBIDDEN
    );
    let windows = snupkg(
        "Acme.Json",
        "1.0.0",
        &[("a.pdb", b"Microsoft C/C++ MSF 7.00\r\n\x1aDS".to_vec())],
    );
    assert_eq!(symbol_push(&push, windows).await, StatusCode::BAD_REQUEST);
    let mismatch = snupkg("Acme.Json", "9.9.9", &[("a.pdb", pdb.clone())]);
    assert_eq!(symbol_push(&push, mismatch).await, StatusCode::NOT_FOUND);

    let (status, _, body) = call(
        &app,
        Method::PUT,
        "/api/v2/symbolpackage",
        Some(&push),
        Some(package.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body:?}");
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap()["symbols"],
        1
    );
    assert_eq!(
        symbol_push(&push, package.clone()).await,
        StatusCode::CONFLICT
    );

    // Anonymous debuggers fetch by key; casing differs between clients.
    let (status, headers, body) = call(
        &app,
        Method::GET,
        &symbol_path("acme.json.pdb", KEY),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.as_ref(), pdb.as_slice());
    assert_eq!(
        headers[header::CACHE_CONTROL],
        "public, max-age=31536000, immutable"
    );
    let upper = symbol_path("ACME.Json.PDB", &KEY.to_uppercase());
    assert_eq!(
        call(&app, Method::GET, &upper, None, None).await.0,
        StatusCode::OK
    );
    let (status, headers, body) = call(
        &app,
        Method::HEAD,
        &symbol_path("acme.json.pdb", KEY),
        None,
        None,
    )
    .await;
    assert_eq!((status, body.len()), (StatusCode::OK, 0));
    assert_eq!(headers[header::CONTENT_LENGTH], pdb.len().to_string());
    for path in [
        "/symbols/acme.json.pdb/03020100050407060809\
0a0b0c0d0e0fffffffff/other.pdb"
            .to_string(),
        symbol_path("acme.json.pdb", &"0".repeat(40)),
        symbol_path("acme.json.pdb", "short"),
        symbol_path("acme.json.dll", KEY),
    ] {
        assert_eq!(
            call(&app, Method::GET, &path, None, None).await.0,
            StatusCode::NOT_FOUND,
            "{path}"
        );
    }

    let (_, _, details) = call(&app, Method::GET, "/api/packages/acme.json", None, None).await;
    let details: Value = serde_json::from_slice(&details).unwrap();
    assert_eq!(details["versions"][0]["symbols"]["files"], 1);

    // Replacing symbols needs an explicit delete by someone entitled to it.
    let delete = |credential: Option<&str>| {
        let app = app.clone();
        let credential = credential.map(str::to_owned);
        async move {
            call(
                &app,
                Method::DELETE,
                "/api/packages/acme.json/1.0.0/symbols",
                credential.as_deref(),
                None,
            )
            .await
            .0
        }
    };
    assert_eq!(delete(None).await, StatusCode::UNAUTHORIZED);
    assert_eq!(delete(Some(&intruder)).await, StatusCode::FORBIDDEN);
    assert_eq!(delete(Some(&narrow)).await, StatusCode::FORBIDDEN);
    assert_eq!(delete(Some(&push)).await, StatusCode::NO_CONTENT);
    assert_eq!(delete(Some(&push)).await, StatusCode::NOT_FOUND);
    assert_eq!(
        call(
            &app,
            Method::GET,
            &symbol_path("acme.json.pdb", KEY),
            None,
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let queued: i64 = sqlx::query_scalar("SELECT count(*) FROM storage_cleanup")
        .fetch_one(&state.db)
        .await
        .unwrap();
    assert_eq!(
        queued, 1,
        "the removed PDB blob waits in the delayed cleanup queue"
    );
    let replacement = snupkg(
        "Acme.Json",
        "1.0.0",
        &[("Acme.Json.pdb", portable_pdb(pdb_id(), 7))],
    );
    assert_eq!(symbol_push(&push, replacement).await, StatusCode::CREATED);
    let (_, _, body) = call(
        &app,
        Method::GET,
        &symbol_path("acme.json.pdb", KEY),
        None,
        None,
    )
    .await;
    assert_eq!(body.len(), portable_pdb(pdb_id(), 7).len());

    // Both mutations reach the audit log with their own event types.
    let (_, _, events) = call(&app, Method::GET, "/api/events", Some(ADMIN), None).await;
    let events: Value = serde_json::from_slice(&events).unwrap();
    let types: Vec<&str> = events
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["type"].as_str())
        .collect();
    assert!(
        types.contains(&"symbols.published") && types.contains(&"symbols.deleted"),
        "{types:?}"
    );
}

#[tokio::test]
#[ignore = "requires Docker PostgreSQL"]
async fn private_feed_symbols_follow_feed_access() {
    let (app, _state, _dir) = app().await;
    let team = profile(&app, "Team").await;
    let reader_profile = profile(&app, "Reader").await;
    let outsider_profile = profile(&app, "Outsider").await;
    let feed = json_call(&app, Method::POST, "/api/feeds", json!({"name":"secure"})).await;
    let feed_id = feed["id"].as_str().unwrap();
    for (profile, role) in [(&team, "publisher"), (&reader_profile, "reader")] {
        json_call(
            &app,
            Method::PUT,
            &format!("/api/feeds/{feed_id}/members/{profile}"),
            json!({"role":role}),
        )
        .await;
    }
    let publisher = key(&app, &team, &["package:push", "package:read"], "").await;
    let reader = key(&app, &reader_profile, &["package:read"], "").await;
    let prefixed = key(&app, &reader_profile, &["package:read"], "elsewhere.").await;
    let outsider = key(&app, &outsider_profile, &["package:read"], "").await;
    let pdb = portable_pdb(pdb_id(), 33);
    assert_eq!(
        call(
            &app,
            Method::PUT,
            "/feeds/secure/api/v2/package",
            Some(&publisher),
            Some(nupkg("Team.Lib", "2.0.0"))
        )
        .await
        .0,
        StatusCode::CREATED
    );
    let package = snupkg("Team.Lib", "2.0.0", &[("Team.Lib.pdb", pdb.clone())]);
    assert_eq!(
        call(
            &app,
            Method::PUT,
            "/feeds/secure/api/v2/symbolpackage",
            Some(&reader),
            Some(package.clone())
        )
        .await
        .0,
        StatusCode::FORBIDDEN,
        "a reader cannot publish symbols"
    );
    let (status, _, body) = call(
        &app,
        Method::PUT,
        "/feeds/secure/api/v2/symbolpackage",
        Some(&publisher),
        Some(package),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body:?}");

    let path = "/feeds/secure/symbols/team.lib.pdb/03020100050407060809\
0a0b0c0d0e0fffffffff/team.lib.pdb";
    assert_eq!(
        call(&app, Method::GET, path, None, None).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&app, Method::GET, path, Some(&outsider), None).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(&app, Method::GET, path, Some(&prefixed), None).await.0,
        StatusCode::NOT_FOUND
    );
    let (status, headers, body) = call(&app, Method::GET, path, Some(&reader), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.as_ref(), pdb.as_slice());
    assert_eq!(headers[header::CACHE_CONTROL], "private, max-age=3600");
    // The root registry never sees feed symbols.
    assert_eq!(
        call(
            &app,
            Method::GET,
            &symbol_path("team.lib.pdb", KEY),
            None,
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
}
