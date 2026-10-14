use axum::{
    Router,
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use nushelf::{AppState, config::Config};
use reqwest::Client;
use serde_json::{Value, json};
use sha2::{Digest, Sha512};
use sqlx::PgPool;
use std::{
    collections::HashMap,
    io::{Cursor, Write},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use uuid::Uuid;

const ADMIN: &str = "upstream-test-administrator-at-least-32-characters";
#[derive(Clone)]
struct Mock {
    base: String,
    packages: Arc<HashMap<String, Bytes>>,
    downloads: Arc<AtomicUsize>,
    indices: Arc<AtomicUsize>,
    versions: Arc<AtomicUsize>,
    required_token: Option<String>,
    offline: Arc<std::sync::atomic::AtomicBool>,
}
fn fixture(id: &str) -> Bytes {
    let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
    archive
        .start_file("test.nuspec", zip::write::SimpleFileOptions::default())
        .unwrap();
    write!(archive,"<package><metadata><id>{id}</id><version>1.0.0</version><authors>Proxy Team</authors><description>A proxy integration fixture.</description><tags>proxy test</tags></metadata></package>").unwrap();
    archive
        .start_file("lib/net8.0/_._", zip::write::SimpleFileOptions::default())
        .unwrap();
    archive.write_all(b"").unwrap();
    Bytes::from(archive.finish().unwrap().into_inner())
}
fn permitted(mock: &Mock, headers: &HeaderMap) -> bool {
    mock.required_token.as_ref().is_none_or(|token| {
        headers.get("authorization").and_then(|h| h.to_str().ok())
            == Some(format!("Bearer {token}").as_str())
    })
}
async fn index(State(mock): State<Mock>, headers: HeaderMap) -> Response {
    if mock.offline.load(Ordering::Relaxed) {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    if !permitted(&mock, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    mock.indices.fetch_add(1, Ordering::Relaxed);
    axum::Json(json!({"version":"3.0.0","resources":[{"@type":"PackageBaseAddress/3.0.0","@id":format!("{}/flat/",mock.base)},{"@type":"RegistrationsBaseUrl/3.6.0","@id":format!("{}/reg/",mock.base)}]})).into_response()
}
async fn versions(
    State(mock): State<Mock>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if mock.offline.load(Ordering::Relaxed) {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    if !permitted(&mock, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    mock.versions.fetch_add(1, Ordering::Relaxed);
    if mock.packages.contains_key(&id) {
        axum::Json(json!({"versions":["1.0.0"]})).into_response()
    } else {
        StatusCode::NOT_FOUND.into_response()
    }
}
async fn archive(
    State(mock): State<Mock>,
    Path((id, version, file)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> Response {
    if mock.offline.load(Ordering::Relaxed) {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    if !permitted(&mock, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Some(bytes) = mock.packages.get(&id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if version != "1.0.0" {
        return StatusCode::NOT_FOUND.into_response();
    }
    if file.ends_with(".sha512") {
        let hash = if id == "broken.hash" {
            STANDARD.encode([0u8; 64])
        } else {
            STANDARD.encode(Sha512::digest(bytes))
        };
        return hash.into_response();
    }
    mock.downloads.fetch_add(1, Ordering::Relaxed);
    tokio::time::sleep(Duration::from_millis(40)).await;
    bytes.clone().into_response()
}
async fn registration(
    State(mock): State<Mock>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if mock.offline.load(Ordering::Relaxed) {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    if !permitted(&mock, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if !mock.packages.contains_key(&id) {
        return StatusCode::NOT_FOUND.into_response();
    }
    axum::Json(json!({"count":1,"items":[{"count":1,"items":[{"catalogEntry":{"id":id,"version":"1.0.0","authors":"Proxy Team","description":"A proxy integration fixture.","tags":"proxy test","listed":true,"published":"2026-01-01T00:00:00Z","dependencyGroups":[]}}]}]})).into_response()
}
async fn origin(ids: &[&str], token: Option<String>) -> (Mock, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mock = Mock {
        base: format!("http://{}", listener.local_addr().unwrap()),
        packages: Arc::new(
            ids.iter()
                .map(|id| {
                    (
                        id.to_ascii_lowercase(),
                        fixture(if *id == "Broken.Identity" {
                            "Wrong.Identity"
                        } else {
                            id
                        }),
                    )
                })
                .collect(),
        ),
        downloads: Arc::new(AtomicUsize::new(0)),
        indices: Arc::new(AtomicUsize::new(0)),
        versions: Arc::new(AtomicUsize::new(0)),
        required_token: token,
        offline: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    };
    let app = Router::new()
        .route("/v3/index.json", get(index))
        .route("/flat/{id}/index.json", get(versions))
        .route("/flat/{id}/{version}/{file}", get(archive))
        .route("/reg/{id}/index.json", get(registration))
        .with_state(mock.clone());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (mock, task)
}
async fn response(response: reqwest::Response, status: StatusCode) -> Value {
    let actual = response.status();
    let text = response.text().await.unwrap();
    assert_eq!(actual, status, "{text}");
    serde_json::from_str(&text).unwrap()
}
async fn source(
    client: &Client,
    base: &str,
    name: &str,
    mock: &Mock,
    priority: i32,
    patterns: Vec<&str>,
    access: (bool, Vec<&str>),
) -> String {
    let (private, profiles) = access;
    let result=response(client.post(format!("{base}/api/upstreams")).bearer_auth(ADMIN).json(&json!({"name":name,"url":format!("{}/v3/index.json",mock.base),"priority":priority,"patterns":patterns,"private":private,"allowedProfiles":profiles,"allowHttp":true,"allowPrivateNetwork":true,"metadataTtlSeconds":1,"authType":if mock.required_token.is_some(){"bearer"}else{"none"},"credentialEnv":mock.required_token.as_ref().map(|_|"UPSTREAM_TEST_TOKEN")})).send().await.unwrap(),StatusCode::OK).await;
    result["id"].as_str().unwrap().to_owned()
}

#[tokio::test]
#[ignore = "requires Docker PostgreSQL and MinIO"]
async fn multi_source_read_through_cache_and_private_access() {
    let connection = std::env::var("TEST_DATABASE_URL").unwrap();
    let root = PgPool::connect(&connection).await.unwrap();
    for provider in ["local", "s3"] {
        let name = format!("nushelf_upstream_{}", Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE DATABASE {name}"))
            .execute(&root)
            .await
            .unwrap();
        let mut url = url::Url::parse(&connection).unwrap();
        url.set_path(&format!("/{name}"));
        let db = PgPool::connect(url.as_str()).await.unwrap();
        sqlx::migrate!("./migrations").run(&db).await.unwrap();
        let dir = tempfile::tempdir().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let config = Config {
            database_url: url.to_string(),
            public_url: base.clone(),
            bind: listener.local_addr().unwrap(),
            admin_key: ADMIN.into(),
            max_package_bytes: 1024 * 1024,
            database_connections: 20,
            storage_provider: provider.into(),
            storage_path: dir.path().to_string_lossy().into_owned(),
            metrics_bind: None,
        };
        let storage: Arc<dyn object_store::ObjectStore> = if provider == "local" {
            nushelf::storage::build(&config).unwrap()
        } else {
            Arc::new(
                object_store::aws::AmazonS3Builder::new()
                    .with_bucket_name("nushelf-test")
                    .with_region("us-east-1")
                    .with_endpoint(std::env::var("TEST_S3_ENDPOINT").unwrap())
                    .with_access_key_id("test-user")
                    .with_secret_access_key("test-only-password")
                    .with_allow_http(true)
                    .build()
                    .unwrap(),
            )
        };
        let state = AppState {
            db: db.clone(),
            storage: storage.clone(),
            config: Arc::new(config),
            upload_slots: Arc::new(tokio::sync::Semaphore::new(4)),
            feeds: Arc::default(),
            feed: None,
            read_prefix: String::new(),
        };
        let app = tokio::spawn(async move {
            axum::serve(listener, nushelf::router(state)).await.unwrap();
        });
        let client = Client::new();
        let (primary, primary_task) = origin(
            &[
                "Public.Shared",
                "Public.Concurrent",
                "Broken.Identity",
                "Broken.Hash",
            ],
            None,
        )
        .await;
        let (secondary, secondary_task) =
            origin(&["Public.Secondary", "Public.Shared"], None).await;
        let (private, private_task) = origin(
            &["Secret.Core"],
            Some(std::env::var("UPSTREAM_TEST_TOKEN").unwrap()),
        )
        .await;
        let first = source(
            &client,
            &base,
            "Primary",
            &primary,
            10,
            vec!["*"],
            (false, vec![]),
        )
        .await;
        source(
            &client,
            &base,
            "Secondary",
            &secondary,
            20,
            vec!["*"],
            (false, vec![]),
        )
        .await;
        response(
            client
                .post(format!("{base}/api/upstreams/{first}/test"))
                .bearer_auth(ADMIN)
                .send()
                .await
                .unwrap(),
            StatusCode::OK,
        )
        .await;
        let blocked=response(client.post(format!("{base}/api/upstreams/test")).bearer_auth(ADMIN).json(&json!({"name":"Unsafe","url":format!("{}/v3/index.json",primary.base),"allowHttp":true})).send().await.unwrap(),StatusCode::BAD_GATEWAY).await;
        assert!(
            blocked["error"]
                .as_str()
                .unwrap()
                .contains("Private/reserved")
        );
        let index = response(
            client
                .get(format!("{base}/v3/flatcontainer/public.shared/index.json"))
                .send()
                .await
                .unwrap(),
            StatusCode::OK,
        )
        .await;
        assert_eq!(index["versions"], json!(["1.0.0"]));
        let shared =
            format!("{base}/v3/flatcontainer/public.shared/1.0.0/public.shared.1.0.0.nupkg");
        let downloaded = client.get(&shared).send().await.unwrap();
        assert_eq!(downloaded.headers()["x-nushelf-cache"], "MISS");
        assert_eq!(
            downloaded.bytes().await.unwrap(),
            primary.packages["public.shared"]
        );
        let cached = client.get(&shared).send().await.unwrap();
        assert_eq!(cached.headers()["x-nushelf-cache"], "HIT");
        assert_eq!(
            cached.bytes().await.unwrap(),
            primary.packages["public.shared"]
        );
        assert_eq!(primary.downloads.load(Ordering::Relaxed), 1);
        assert_eq!(secondary.downloads.load(Ordering::Relaxed), 0);
        let reg = response(
            client
                .get(format!("{base}/v3/registration/public.shared/index.json"))
                .send()
                .await
                .unwrap(),
            StatusCode::OK,
        )
        .await;
        assert!(reg.to_string().contains(&base));
        assert!(!reg.to_string().contains(&primary.base));
        let details = response(
            client
                .get(format!("{base}/api/packages/public.shared"))
                .send()
                .await
                .unwrap(),
            StatusCode::OK,
        )
        .await;
        assert_eq!(details["cacheStatus"], "cached");
        assert_eq!(details["upstream"]["name"], "Primary");
        let search = response(
            client
                .get(format!(
                    "{base}/v3/search?q=public.shared&semVerLevel=2.0.0"
                ))
                .send()
                .await
                .unwrap(),
            StatusCode::OK,
        )
        .await;
        assert_eq!(search["totalHits"], 1);
        assert_eq!(search["data"][0]["cacheStatus"], "cached");
        let fallback = client
            .get(format!(
                "{base}/v3/flatcontainer/public.secondary/1.0.0/public.secondary.1.0.0.nupkg"
            ))
            .send()
            .await
            .unwrap();
        assert_eq!(fallback.status(), StatusCode::OK);
        assert_eq!(secondary.downloads.load(Ordering::Relaxed), 1);
        let before =
            primary.versions.load(Ordering::Relaxed) + secondary.versions.load(Ordering::Relaxed);
        for _ in 0..2 {
            assert_eq!(
                client
                    .get(format!(
                        "{base}/v3/flatcontainer/missing.package/index.json"
                    ))
                    .send()
                    .await
                    .unwrap()
                    .status(),
                StatusCode::NOT_FOUND
            );
        }
        assert_eq!(
            primary.versions.load(Ordering::Relaxed) + secondary.versions.load(Ordering::Relaxed),
            before + 2
        );
        let concurrent = format!(
            "{base}/v3/flatcontainer/public.concurrent/1.0.0/public.concurrent.1.0.0.nupkg"
        );
        let mut requests = Vec::new();
        for _ in 0..16 {
            let client = client.clone();
            let url = concurrent.clone();
            requests.push(tokio::spawn(async move {
                client.get(url).send().await.unwrap().status()
            }));
        }
        for request in requests {
            assert_eq!(request.await.unwrap(), StatusCode::OK);
        }
        assert_eq!(primary.downloads.load(Ordering::Relaxed), 2);
        for id in ["broken.identity", "broken.hash"] {
            assert_eq!(
                client
                    .get(format!(
                        "{base}/v3/flatcontainer/{id}/1.0.0/{id}.1.0.0.nupkg"
                    ))
                    .send()
                    .await
                    .unwrap()
                    .status(),
                StatusCode::BAD_GATEWAY
            );
        }
        let bad_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM upstream_cache WHERE package_id LIKE 'broken.%'",
        )
        .fetch_one(&db)
        .await
        .unwrap();
        assert_eq!(bad_count, 0);
        let mut ids = Vec::new();
        let mut keys = Vec::new();
        for owner in ["Allowed", "Denied"] {
            let profile = response(
                client
                    .post(format!("{base}/api/profiles"))
                    .bearer_auth(ADMIN)
                    .json(&json!({"name":owner}))
                    .send()
                    .await
                    .unwrap(),
                StatusCode::OK,
            )
            .await;
            let id = profile["id"].as_str().unwrap().to_owned();
            let key=response(client.post(format!("{base}/api/profiles/{id}/keys")).bearer_auth(ADMIN).json(&json!({"label":"Reader","scopes":["package:read","mcp:read","package:push"],"packagePrefix":""})).send().await.unwrap(),StatusCode::OK).await;
            ids.push(id);
            keys.push(key["key"].as_str().unwrap().to_owned());
        }
        let private_id = source(
            &client,
            &base,
            "Private",
            &private,
            1,
            vec!["secret.*"],
            (true, vec![&ids[0]]),
        )
        .await;
        let private_url =
            format!("{base}/v3/flatcontainer/secret.core/1.0.0/secret.core.1.0.0.nupkg");
        let denied = client.get(&private_url).send().await.unwrap();
        assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
        assert!(denied.headers().contains_key("www-authenticate"));
        assert_eq!(private.downloads.load(Ordering::Relaxed), 0);
        assert_eq!(
            client
                .get(&private_url)
                .bearer_auth(&keys[1])
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        let private_download = client
            .get(&private_url)
            .basic_auth("nushelf", Some(&keys[0]))
            .send()
            .await
            .unwrap();
        assert_eq!(private_download.status(), StatusCode::OK);
        assert_eq!(private_download.headers()["x-nushelf-cache"], "MISS");
        assert_eq!(private.downloads.load(Ordering::Relaxed), 1);
        let anonymous = response(
            client
                .get(format!("{base}/v3/search?q=secret.core"))
                .send()
                .await
                .unwrap(),
            StatusCode::OK,
        )
        .await;
        assert_eq!(anonymous["totalHits"], 0);
        let visible = response(
            client
                .get(format!("{base}/v3/search?q=secret.core"))
                .bearer_auth(&keys[0])
                .send()
                .await
                .unwrap(),
            StatusCode::OK,
        )
        .await;
        assert_eq!(visible["totalHits"], 1);
        let tool=response(client.post(format!("{base}/mcp")).bearer_auth(&keys[0]).header("Accept","application/json, text/event-stream").header("MCP-Protocol-Version","2025-06-18").json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"get_package","arguments":{"id":"secret.core"}}})).send().await.unwrap(),StatusCode::OK).await;
        assert_eq!(tool["result"]["isError"], false);
        let leaked = response(
            client
                .get(format!("{base}/api/stats"))
                .send()
                .await
                .unwrap(),
            StatusCode::OK,
        )
        .await;
        assert_eq!(leaked["cachedPackages"], 3);
        assert_eq!(
            client
                .put(format!("{base}/api/v2/package"))
                .bearer_auth(&keys[0])
                .body(fixture("Public.Shared"))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::CONFLICT
        );
        response(
            client
                .put(format!("{base}/api/v2/package"))
                .bearer_auth(&keys[0])
                .body(fixture("Local.Owned"))
                .send()
                .await
                .unwrap(),
            StatusCode::CREATED,
        )
        .await;
        let upstream_before = primary.versions.load(Ordering::Relaxed);
        assert_eq!(
            client
                .get(format!(
                    "{base}/v3/flatcontainer/local.owned/2.0.0/local.owned.2.0.0.nupkg"
                ))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(primary.versions.load(Ordering::Relaxed), upstream_before);
        primary.offline.store(true, Ordering::Relaxed);
        private.offline.store(true, Ordering::Relaxed);
        tokio::time::sleep(Duration::from_millis(1100)).await;
        assert_eq!(
            client.get(&shared).send().await.unwrap().status(),
            StatusCode::OK
        );
        let offline = response(
            client
                .get(format!("{base}/v3/flatcontainer/public.shared/index.json"))
                .send()
                .await
                .unwrap(),
            StatusCode::OK,
        )
        .await;
        assert_eq!(offline["versions"], json!(["1.0.0"]));
        assert_eq!(
            client
                .get(&private_url)
                .bearer_auth(&keys[0])
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            client.get(&private_url).send().await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        let sources = response(
            client
                .get(format!("{base}/api/upstreams"))
                .bearer_auth(ADMIN)
                .send()
                .await
                .unwrap(),
            StatusCode::OK,
        )
        .await;
        assert!(!sources.to_string().contains("upstream-fixture-secret"));
        assert!(!sources.to_string().contains("service_index"));
        assert_eq!(
            client
                .get(format!("{base}/api/upstreams/cache"))
                .bearer_auth(&keys[0])
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        sqlx::query("UPDATE upstream_cache SET cached_at=now()-interval '2 hours',last_accessed_at=now()-interval '40 days'").execute(&db).await.unwrap();
        assert_eq!(
            client
                .put(format!("{base}/api/cache/secret.core/1.0.0/protection"))
                .bearer_auth(ADMIN)
                .json(&json!({"protected":true}))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            client
                .patch(format!("{base}/api/cache/policy"))
                .bearer_auth(ADMIN)
                .json(&json!({"enabled":false,"maxBytes":1,"idleDays":30}))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            client
                .post(format!("{base}/api/cache/maintenance"))
                .json(&json!({}))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM upstream_cache")
            .fetch_one(&db)
            .await
            .unwrap();
        let preview = response(
            client
                .post(format!("{base}/api/cache/maintenance"))
                .bearer_auth(ADMIN)
                .json(&json!({"dryRun":true,"orphans":true}))
                .send()
                .await
                .unwrap(),
            StatusCode::OK,
        )
        .await;
        assert!(!preview["selected"].as_array().unwrap().is_empty());
        assert!(
            preview["selected"]
                .as_array()
                .unwrap()
                .iter()
                .all(|p| p["id"] != "secret.core")
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM upstream_cache")
                .fetch_one(&db)
                .await
                .unwrap(),
            count
        );
        let removed = response(
            client
                .post(format!("{base}/api/cache/maintenance"))
                .bearer_auth(ADMIN)
                .json(&json!({"dryRun":false}))
                .send()
                .await
                .unwrap(),
            StatusCode::OK,
        )
        .await;
        assert_eq!(removed["deletedObjects"], 0);
        assert_eq!(removed["quotaSatisfied"], false);
        assert!(
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM upstream_metadata WHERE package_id='public.shared')"
            )
            .fetch_one(&db)
            .await
            .unwrap()
        );
        assert!(
            !sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM upstream_cache WHERE package_id='public.shared')"
            )
            .fetch_one(&db)
            .await
            .unwrap()
        );
        let local_key: String = sqlx::query_scalar(
            "SELECT storage_key FROM package_versions WHERE package_id='local.owned'",
        )
        .fetch_one(&db)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO storage_cleanup(storage_key,not_before) VALUES($1,now()-interval '1 day')",
        )
        .bind(&local_key)
        .execute(&db)
        .await
        .unwrap();
        sqlx::query("UPDATE storage_cleanup SET not_before=now()-interval '1 day'")
            .execute(&db)
            .await
            .unwrap();
        let cleanup = response(
            client
                .post(format!("{base}/api/cache/maintenance"))
                .bearer_auth(ADMIN)
                .json(&json!({"dryRun":false}))
                .send()
                .await
                .unwrap(),
            StatusCode::OK,
        )
        .await;
        assert!(cleanup["deletedObjects"].as_i64().unwrap() > 0);
        assert!(
            storage
                .head(&object_store::path::Path::from(local_key))
                .await
                .is_ok()
        );
        let purged = response(
            client
                .delete(format!("{base}/api/upstreams/{private_id}/cache"))
                .bearer_auth(ADMIN)
                .send()
                .await
                .unwrap(),
            StatusCode::OK,
        )
        .await;
        assert_eq!(purged["clearedVersions"], 1);
        assert_eq!(purged["storageCleanupFailures"], 0);
        response(
            client
                .delete(format!("{base}/api/upstreams/{private_id}"))
                .bearer_auth(ADMIN)
                .send()
                .await
                .unwrap(),
            StatusCode::OK,
        )
        .await;
        app.abort();
        primary_task.abort();
        secondary_task.abort();
        private_task.abort();
        db.close().await;
        sqlx::query(&format!("DROP DATABASE {name} WITH (FORCE)"))
            .execute(&root)
            .await
            .unwrap();
        println!(
            "PASS {provider}: priority fallback, pinning, metadata, checksum, deduplication, negative cache, private ACL/Basic/MCP, offline cache and cleanup"
        );
    }
    root.close().await;
}
