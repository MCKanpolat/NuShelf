use axum::http::StatusCode;
use bytes::Bytes;
use nushelf::{AppState, config::Config};
use reqwest::{Client, Response};
use serde_json::{Value, json};
use sqlx::{PgPool, postgres::PgPoolOptions};
use std::{
    io::{Cursor, Write},
    sync::Arc,
};
use uuid::Uuid;

const ADMIN: &str = "integration-only-admin-secret-at-least-32-characters";
fn fixture(id: &str, version: &str) -> Bytes {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    writer
        .start_file("package.nuspec", zip::write::SimpleFileOptions::default())
        .unwrap();
    write!(writer,"<package><metadata><id>{id}</id><version>{version}</version><authors>Team</authors><description>A JSON parser for testing.</description><tags>json testing</tags><dependencies><group targetFramework=\"net8.0\"><dependency id=\"Example.Dependency\" version=\"[1.0.0,2.0.0)\"/></group></dependencies></metadata></package>").unwrap();
    writer
        .start_file("lib/net8.0/_._", zip::write::SimpleFileOptions::default())
        .unwrap();
    writer.write_all(b"").unwrap();
    Bytes::from(writer.finish().unwrap().into_inner())
}
async fn value(response: Response, expected: StatusCode) -> Value {
    let status = response.status();
    let text = response.text().await.unwrap();
    assert_eq!(status, expected, "{text}");
    serde_json::from_str(&text).unwrap()
}
async fn key(client: &Client, base: &str, profile: &str, scopes: &[&str], prefix: &str) -> String {
    value(
        client
            .post(format!("{base}/api/profiles/{profile}/keys"))
            .bearer_auth(ADMIN)
            .json(&json!({"label":"test-key","scopes":scopes,"packagePrefix":prefix}))
            .send()
            .await
            .unwrap(),
        StatusCode::OK,
    )
    .await["key"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
#[ignore = "requires Docker PostgreSQL and MinIO; run compose.test.yml"]
async fn postgres_registry_local_and_s3() {
    let database_url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    let root = PgPool::connect(&database_url).await.unwrap();
    for provider in ["local", "s3"] {
        let db_name = format!("nushelf_it_{}", Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE DATABASE {db_name}"))
            .execute(&root)
            .await
            .unwrap();
        let mut db_url = url::Url::parse(&database_url).unwrap();
        db_url.set_path(&format!("/{db_name}"));
        let db = PgPoolOptions::new()
            .max_connections(8)
            .connect(db_url.as_str())
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&db).await.unwrap();
        let dir = tempfile::tempdir().unwrap();
        let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let config = Config {
            database_url: db_url.to_string(),
            public_url: base.clone(),
            bind: listener.local_addr().unwrap(),
            admin_key: ADMIN.into(),
            max_package_bytes: 2 * 1024 * 1024,
            database_connections: 8,
            storage_provider: provider.into(),
            storage_path: dir.path().to_str().unwrap().into(),
        };
        let storage: Arc<dyn object_store::ObjectStore> = if provider == "local" {
            nushelf::storage::build(&config).unwrap()
        } else {
            Arc::new(
                object_store::aws::AmazonS3Builder::new()
                    .with_bucket_name("nushelf-test")
                    .with_region("us-east-1")
                    .with_access_key_id("test-user")
                    .with_secret_access_key("test-only-password")
                    .with_endpoint(std::env::var("TEST_S3_ENDPOINT").expect("TEST_S3_ENDPOINT"))
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
        let server = tokio::spawn(async move {
            axum::serve(listener, nushelf::router(state)).await.unwrap();
        });
        let client = Client::new();
        let index = value(
            client
                .get(format!("{base}/v3/index.json"))
                .send()
                .await
                .unwrap(),
            StatusCode::OK,
        )
        .await;
        assert_eq!(index["version"], "3.0.0");
        assert_eq!(index["resources"].as_array().unwrap().len(), 6);
        let readiness = value(
            client
                .get(format!("{base}/health/ready"))
                .send()
                .await
                .unwrap(),
            StatusCode::OK,
        )
        .await;
        assert_eq!(
            readiness,
            json!({"checks":{"database":"ok","storage":"ok"}})
        );
        if provider == "s3" {
            // A bucket that does not exist must fail readiness, not look like an empty store.
            let missing_bucket: Arc<dyn object_store::ObjectStore> = Arc::new(
                object_store::aws::AmazonS3Builder::new()
                    .with_bucket_name(format!("nushelf-missing-{}", Uuid::new_v4().simple()))
                    .with_region("us-east-1")
                    .with_access_key_id("test-user")
                    .with_secret_access_key("test-only-password")
                    .with_endpoint(std::env::var("TEST_S3_ENDPOINT").expect("TEST_S3_ENDPOINT"))
                    .with_allow_http(true)
                    .build()
                    .unwrap(),
            );
            let broken_listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
            let broken_base = format!(
                "http://127.0.0.1:{}",
                broken_listener.local_addr().unwrap().port()
            );
            let broken_config = Config {
                database_url: db_url.to_string(),
                public_url: broken_base.clone(),
                bind: broken_listener.local_addr().unwrap(),
                admin_key: ADMIN.into(),
                max_package_bytes: 2 * 1024 * 1024,
                database_connections: 8,
                storage_provider: "s3".into(),
                storage_path: dir.path().to_str().unwrap().into(),
            };
            let broken = AppState {
                db: db.clone(),
                storage: missing_bucket,
                config: Arc::new(broken_config),
                upload_slots: Arc::new(tokio::sync::Semaphore::new(1)),
                feeds: Arc::default(),
                feed: None,
                read_prefix: String::new(),
            };
            let broken_server = tokio::spawn(async move {
                axum::serve(broken_listener, nushelf::router(broken))
                    .await
                    .unwrap();
            });
            let unready = value(
                client
                    .get(format!("{broken_base}/health/ready"))
                    .send()
                    .await
                    .unwrap(),
                StatusCode::SERVICE_UNAVAILABLE,
            )
            .await;
            assert_eq!(
                unready,
                json!({"checks":{"database":"ok","storage":"unavailable"}})
            );
            broken_server.abort();
        }
        assert_eq!(
            client
                .get(format!("{base}/manage"))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            client
                .get(format!("{base}/api/profiles"))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        let profile = value(
            client
                .post(format!("{base}/api/profiles"))
                .bearer_auth(ADMIN)
                .json(&json!({"name":"Test Team"}))
                .send()
                .await
                .unwrap(),
            StatusCode::OK,
        )
        .await;
        let profile_id = profile["id"].as_str().unwrap();
        let credential = key(
            &client,
            &base,
            profile_id,
            &["package:push", "package:unlist", "mcp:read", "keys:manage"],
            "acme.",
        )
        .await;
        let readonly = key(&client, &base, profile_id, &["mcp:read"], "").await;
        let p = fixture("Acme.Json", "1.0.0");
        assert_eq!(
            client
                .put(format!("{base}/api/v2/package"))
                .body(p.clone())
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            client
                .put(format!("{base}/api/v2/package"))
                .bearer_auth(&readonly)
                .body(p.clone())
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            client
                .put(format!("{base}/api/v2/package"))
                .bearer_auth(&credential)
                .body(fixture("Other.Package", "1.0.0"))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        value(
            client
                .put(format!("{base}/api/v2/package"))
                .header("X-NuGet-ApiKey", &credential)
                .body(p.clone())
                .send()
                .await
                .unwrap(),
            StatusCode::CREATED,
        )
        .await;
        assert_eq!(
            client
                .put(format!("{base}/api/v2/package"))
                .bearer_auth(&credential)
                .body(p.clone())
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::CONFLICT
        );
        for version in ["1.0.10", "1.0.2", "2.0.0-beta.2"] {
            let multipart = reqwest::multipart::Form::new().part(
                "package",
                reqwest::multipart::Part::bytes(fixture("Acme.Json", version).to_vec())
                    .file_name("test.nupkg"),
            );
            value(
                client
                    .put(format!("{base}/api/v2/package"))
                    .bearer_auth(&credential)
                    .multipart(multipart)
                    .send()
                    .await
                    .unwrap(),
                StatusCode::CREATED,
            )
            .await;
        }
        let versions = value(
            client
                .get(format!("{base}/v3/flatcontainer/acme.json/index.json"))
                .send()
                .await
                .unwrap(),
            StatusCode::OK,
        )
        .await;
        assert_eq!(
            versions["versions"],
            json!(["1.0.0", "1.0.2", "1.0.10", "2.0.0-beta.2"])
        );
        let search = value(
            client
                .get(format!("{base}/v3/search?q=json&semVerLevel=2.0.0"))
                .send()
                .await
                .unwrap(),
            StatusCode::OK,
        )
        .await;
        assert_eq!(search["totalHits"], 1);
        assert_eq!(search["data"][0]["version"], "1.0.10");
        assert_eq!(search["data"][0]["versions"].as_array().unwrap().len(), 3);
        let fuzzy = value(
            client
                .get(format!("{base}/v3/search?q=acme.jso"))
                .send()
                .await
                .unwrap(),
            StatusCode::OK,
        )
        .await;
        assert_eq!(fuzzy["totalHits"], 1);
        let count_only = value(
            client
                .get(format!("{base}/v3/search?take=0"))
                .send()
                .await
                .unwrap(),
            StatusCode::OK,
        )
        .await;
        assert_eq!(count_only["totalHits"], 1);
        assert_eq!(count_only["data"], json!([]));
        let autocomplete = value(
            client
                .get(format!(
                    "{base}/v3/autocomplete?id=acme.json&prerelease=true&semVerLevel=2.0.0"
                ))
                .send()
                .await
                .unwrap(),
            StatusCode::OK,
        )
        .await;
        assert_eq!(
            autocomplete["data"],
            json!(["1.0.0", "1.0.2", "1.0.10", "2.0.0-beta.2"])
        );
        let legacy = value(
            client
                .get(format!("{base}/v3/search?prerelease=true"))
                .send()
                .await
                .unwrap(),
            StatusCode::OK,
        )
        .await;
        assert_eq!(legacy["data"][0]["version"], "1.0.10");
        let pre = value(
            client
                .get(format!(
                    "{base}/v3/search?prerelease=true&semVerLevel=2.0.0"
                ))
                .send()
                .await
                .unwrap(),
            StatusCode::OK,
        )
        .await;
        assert_eq!(pre["data"][0]["version"], "2.0.0-beta.2");
        let wildcard = value(
            client
                .get(format!("{base}/v3/search?q=%25"))
                .send()
                .await
                .unwrap(),
            StatusCode::OK,
        )
        .await;
        assert_eq!(wildcard["totalHits"], 0);
        let registration = value(
            client
                .get(format!("{base}/v3/registration/acme.json/index.json"))
                .send()
                .await
                .unwrap(),
            StatusCode::OK,
        )
        .await;
        assert_eq!(
            registration["items"][0]["items"][0]["catalogEntry"]["dependencyGroups"][0]["dependencies"]
                [0]["range"],
            "[1.0.0,2.0.0)"
        );
        let content = format!("{base}/v3/flatcontainer/acme.json/1.0.0/acme.json.1.0.0.nupkg");
        let head = client.head(&content).send().await.unwrap();
        assert_eq!(head.status(), StatusCode::OK);
        assert_eq!(head.headers()["content-length"], p.len().to_string());
        assert_eq!(
            client
                .get(&content)
                .send()
                .await
                .unwrap()
                .bytes()
                .await
                .unwrap(),
            p
        );
        assert_eq!(
            client
                .get(format!(
                    "{base}/v3/flatcontainer/acme.json/1.0.0/wrong.nupkg"
                ))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            client
                .delete(format!("{base}/api/v2/package/acme.json/1.0.10"))
                .bearer_auth(&credential)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::NO_CONTENT
        );
        let unlisted = value(
            client
                .get(format!("{base}/v3/search?q=acme.json"))
                .send()
                .await
                .unwrap(),
            StatusCode::OK,
        )
        .await;
        assert_eq!(unlisted["data"][0]["version"], "1.0.2");
        assert_eq!(
            client
                .get(format!(
                    "{base}/v3/flatcontainer/acme.json/1.0.10/acme.json.1.0.10.nupkg"
                ))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            client
                .post(format!("{base}/api/v2/package/acme.json/1.0.10"))
                .bearer_auth(&credential)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::NO_CONTENT
        );
        let other = value(
            client
                .post(format!("{base}/api/profiles"))
                .bearer_auth(ADMIN)
                .json(&json!({"name":"Other Team"}))
                .send()
                .await
                .unwrap(),
            StatusCode::OK,
        )
        .await;
        let otherkey = key(
            &client,
            &base,
            other["id"].as_str().unwrap(),
            &["package:push", "package:unlist"],
            "",
        )
        .await;
        assert_eq!(
            client
                .put(format!("{base}/api/v2/package"))
                .bearer_auth(&otherkey)
                .body(fixture("Acme.Json", "3.0.0"))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            client
                .delete(format!("{base}/api/v2/package/acme.json/1.0.0"))
                .bearer_auth(&otherkey)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            client
                .post(format!("{base}/api/profiles/{profile_id}/keys"))
                .bearer_auth(&credential)
                .json(&json!({"label":"escalation","scopes":["package:push"],"packagePrefix":""}))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        let keys = value(
            client
                .get(format!("{base}/api/profiles/{profile_id}/keys"))
                .bearer_auth(&credential)
                .send()
                .await
                .unwrap(),
            StatusCode::OK,
        )
        .await;
        assert!(!keys.to_string().contains(&credential));
        assert!(!keys.to_string().contains("key_hash"));
        let expired = client.post(format!("{base}/api/profiles/{profile_id}/keys")).bearer_auth(ADMIN).json(&json!({"label":"expired","scopes":["mcp:read"],"expiresAt":"2000-01-01T00:00:00Z"})).send().await.unwrap();
        assert_eq!(expired.status(), StatusCode::BAD_REQUEST);
        let mcp = |message: Value| {
            client
                .post(format!("{base}/mcp"))
                .bearer_auth(&readonly)
                .header("Accept", "application/json, text/event-stream")
                .header("MCP-Protocol-Version", "2025-06-18")
                .json(&message)
        };
        let init=value(mcp(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1"}}})).send().await.unwrap(),StatusCode::OK).await;
        assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(
            mcp(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::ACCEPTED
        );
        let tools = value(
            mcp(json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}))
                .send()
                .await
                .unwrap(),
            StatusCode::OK,
        )
        .await;
        assert_eq!(tools["result"]["tools"].as_array().unwrap().len(), 3);
        let called=value(mcp(json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"get_package","arguments":{"id":"Acme.Json"}}})).send().await.unwrap(),StatusCode::OK).await;
        assert_eq!(called["result"]["structuredContent"]["id"], "Acme.Json");
        assert_eq!(
            mcp(json!({"jsonrpc":"2.0","id":4,"method":"ping"}))
                .header("Origin", "https://evil.example")
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            client
                .get(format!("{base}/mcp"))
                .bearer_auth(&readonly)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::METHOD_NOT_ALLOWED
        );
        let race = fixture("Acme.Race", "1.0.0");
        let (a, b) = tokio::join!(
            client
                .put(format!("{base}/api/v2/package"))
                .bearer_auth(&credential)
                .body(race.clone())
                .send(),
            client
                .put(format!("{base}/api/v2/package"))
                .bearer_auth(&credential)
                .body(race)
                .send()
        );
        let mut statuses = vec![a.unwrap().status().as_u16(), b.unwrap().status().as_u16()];
        statuses.sort();
        assert_eq!(statuses, vec![201, 409]);
        let readonly_id = keys
            .as_array()
            .unwrap()
            .iter()
            .find(|k| k["scopes"] == json!(["mcp:read"]))
            .unwrap()["id"]
            .as_str()
            .unwrap();
        value(
            client
                .delete(format!(
                    "{base}/api/profiles/{profile_id}/keys/{readonly_id}"
                ))
                .bearer_auth(ADMIN)
                .send()
                .await
                .unwrap(),
            StatusCode::OK,
        )
        .await;
        assert_eq!(
            client
                .get(format!("{base}/api/me"))
                .bearer_auth(&readonly)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        let stats = value(
            client
                .get(format!("{base}/api/stats"))
                .send()
                .await
                .unwrap(),
            StatusCode::OK,
        )
        .await;
        assert_eq!(stats["packages"], 2);
        assert_eq!(stats["versions"], 5);
        println!(
            "Verified PostgreSQL + {provider}: publish, restore metadata, search, ownership, scopes, unlist/relist, concurrent writes and MCP"
        );
        server.abort();
        db.close().await;
        sqlx::query(&format!("DROP DATABASE {db_name} WITH (FORCE)"))
            .execute(&root)
            .await
            .unwrap();
    }
    root.close().await;
}
