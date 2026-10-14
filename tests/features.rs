use axum::http::StatusCode;
use bytes::Bytes;
use nushelf::{AppState, config::Config};
use reqwest::{Client, Response};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::{
    io::{Cursor, Write},
    sync::Arc,
};
use uuid::Uuid;
const ADMIN: &str = "feature-tests-admin-key-at-least-32-characters";
fn package(id: &str) -> Bytes {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    zip.start_file("package.nuspec", zip::write::SimpleFileOptions::default())
        .unwrap();
    write!(zip,"<package><metadata><id>{id}</id><version>1.0.0</version><authors>Team</authors><description>Feature test</description><readme>README.md</readme><icon>icon.png</icon></metadata></package>").unwrap();
    zip.start_file("README.md", zip::write::SimpleFileOptions::default())
        .unwrap();
    zip.write_all(b"# Feature README\n**Safe content**\n<script>alert(1)</script>")
        .unwrap();
    zip.start_file("icon.png", zip::write::SimpleFileOptions::default())
        .unwrap();
    zip.write_all(b"\x89PNG\r\n\x1a\nfixture").unwrap();
    zip.start_file("lib/net8.0/_._", zip::write::SimpleFileOptions::default())
        .unwrap();
    Bytes::from(zip.finish().unwrap().into_inner())
}
async fn json_response(r: Response) -> Value {
    let status = r.status();
    let body = r.text().await.unwrap();
    assert!(status.is_success(), "{status}: {body}");
    serde_json::from_str(&body).unwrap()
}
async fn profile(c: &Client, b: &str, name: &str) -> String {
    json_response(
        c.post(format!("{b}/api/profiles"))
            .bearer_auth(ADMIN)
            .json(&json!({"name":name}))
            .send()
            .await
            .unwrap(),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .into()
}
async fn key(c: &Client, b: &str, p: &str, scopes: &[&str], prefix: &str) -> String {
    json_response(
        c.post(format!("{b}/api/profiles/{p}/keys"))
            .bearer_auth(ADMIN)
            .json(&json!({"label":"Feature test","scopes":scopes,"packagePrefix":prefix}))
            .send()
            .await
            .unwrap(),
    )
    .await["key"]
        .as_str()
        .unwrap()
        .into()
}
#[tokio::test]
#[ignore = "requires Docker PostgreSQL and MinIO"]
async fn named_feeds_isolate_packages_storage_search_and_permissions() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("nushelf=debug")
        .try_init();
    let database_url = std::env::var("TEST_DATABASE_URL").unwrap();
    let root = PgPool::connect(&database_url).await.unwrap();
    for provider in ["local", "s3"] {
        let database = format!("features_{}", Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE DATABASE {database}"))
            .execute(&root)
            .await
            .unwrap();
        let mut url = url::Url::parse(&database_url).unwrap();
        url.set_path(&format!("/{database}"));
        let db = PgPool::connect(url.as_str()).await.unwrap();
        sqlx::migrate!("./migrations").run(&db).await.unwrap();
        let dir = tempfile::tempdir().unwrap();
        let listener = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let config = Config {
            database_url: url.to_string(),
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
                    .with_endpoint(std::env::var("TEST_S3_ENDPOINT").unwrap())
                    .with_allow_http(true)
                    .build()
                    .unwrap(),
            )
        };
        let state = AppState {
            db: db.clone(),
            storage,
            config: Arc::new(config),
            upload_slots: Arc::new(tokio::sync::Semaphore::new(4)),
            feeds: Arc::default(),
            feed: None,
            read_prefix: String::new(),
        };
        let event_state = state.clone();
        let server =
            tokio::spawn(
                async move { axum::serve(listener, nushelf::router(state)).await.unwrap() },
            );
        let c = Client::new();
        let p = profile(&c, &base, "Publisher").await;
        let other = profile(&c, &base, "Other team").await;
        let publisher = key(
            &c,
            &base,
            &p,
            &[
                "package:push",
                "package:read",
                "package:unlist",
                "package:deprecate",
                "mcp:read",
                "feeds:manage",
            ],
            "",
        )
        .await;
        let reader = key(
            &c,
            &base,
            &p,
            &["package:read", "package:push", "mcp:read"],
            "team.",
        )
        .await;
        let outsider = key(&c, &base, &other, &["package:read", "package:push"], "").await;
        let mut ids = Vec::new();
        for name in ["alpha", "beta"] {
            let f = json_response(
                c.post(format!("{base}/api/feeds"))
                    .bearer_auth(ADMIN)
                    .json(&json!({"name":name}))
                    .send()
                    .await
                    .unwrap(),
            )
            .await;
            let id = f["id"].as_str().unwrap();
            ids.push(id.to_owned());
            assert_eq!(
                c.put(format!("{base}/api/feeds/{id}/members/{p}"))
                    .bearer_auth(ADMIN)
                    .json(&json!({"role":"publisher"}))
                    .send()
                    .await
                    .unwrap()
                    .status(),
                StatusCode::NO_CONTENT
            );
            assert_eq!(
                c.put(format!("{base}/feeds/{name}/api/v2/package"))
                    .bearer_auth(&publisher)
                    .body(package("Team.Core"))
                    .send()
                    .await
                    .unwrap()
                    .status(),
                StatusCode::CREATED
            );
        }
        assert_eq!(
            c.get(format!("{base}/v3/flatcontainer/team.core/index.json"))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
        let feed = format!("{base}/feeds/alpha");
        let dep = json!({"reasons":["Legacy"],"message":"Use the replacement","alternatePackage":{"id":"Team.Next","range":"[2.0.0,)"}});
        assert_eq!(
            c.put(format!("{feed}/api/packages/team.core/1.0.0/deprecation"))
                .bearer_auth(&reader)
                .json(&dep)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            c.put(format!("{feed}/api/packages/team.core/1.0.0/deprecation"))
                .bearer_auth(&publisher)
                .json(&dep)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::NO_CONTENT
        );
        let registration = json_response(
            c.get(format!("{feed}/v3/registration/team.core/index.json"))
                .bearer_auth(&publisher)
                .send()
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(
            registration["items"][0]["items"][0]["catalogEntry"]["deprecation"],
            dep
        );
        assert_eq!(
            registration["items"][0]["items"][0]["catalogEntry"]["listed"],
            true
        );
        assert_eq!(
            c.put(format!("{feed}/api/packages/team.core/1.0.0/deprecation"))
                .bearer_auth(&publisher)
                .json(&json!({"reasons":["BadReason"]}))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            c.delete(format!("{feed}/api/packages/team.core/1.0.0/deprecation"))
                .bearer_auth(&publisher)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::NO_CONTENT
        );
        let registration = json_response(
            c.get(format!("{feed}/v3/registration/team.core/index.json"))
                .bearer_auth(&publisher)
                .send()
                .await
                .unwrap(),
        )
        .await;
        assert!(
            registration["items"][0]["items"][0]["catalogEntry"]
                .get("deprecation")
                .is_none()
        );
        let assets = json_response(
            c.get(format!("{feed}/api/packages/team.core/1.0.0/assets"))
                .bearer_auth(&publisher)
                .send()
                .await
                .unwrap(),
        )
        .await;
        assert!(
            assets["readmeHtml"]
                .as_str()
                .unwrap()
                .contains("<strong>Safe content</strong>")
        );
        assert!(!assets["readmeHtml"].as_str().unwrap().contains("<script"));
        assert_eq!(assets["frameworks"], json!(["net8.0"]));
        let icon = c
            .get(assets["iconUrl"].as_str().unwrap())
            .bearer_auth(&publisher)
            .send()
            .await
            .unwrap();
        assert_eq!(icon.headers()["content-type"], "image/png");
        assert!(icon.bytes().await.unwrap().starts_with(b"\x89PNG"));
        for path in [
            "v3/index.json",
            "v3/search",
            "v3/autocomplete",
            "api/stats",
            "api/packages/team.core",
            "v3/registration/team.core/index.json",
            "v3/flatcontainer/team.core/1.0.0/team.core.1.0.0.nupkg",
        ] {
            assert_eq!(
                c.get(format!("{feed}/{path}"))
                    .send()
                    .await
                    .unwrap()
                    .status(),
                StatusCode::UNAUTHORIZED,
                "{path}"
            );
            assert_eq!(
                c.get(format!("{feed}/{path}"))
                    .bearer_auth(&outsider)
                    .send()
                    .await
                    .unwrap()
                    .status(),
                StatusCode::FORBIDDEN,
                "{path}"
            );
        }
        let index = json_response(
            c.get(format!("{feed}/v3/index.json"))
                .basic_auth("profile", Some(&publisher))
                .send()
                .await
                .unwrap(),
        )
        .await;
        assert!(
            index["resources"]
                .as_array()
                .unwrap()
                .iter()
                .all(|r| r["@id"].as_str().unwrap().starts_with(&feed))
        );
        assert_eq!(
            c.put(format!("{feed}/api/v2/package"))
                .bearer_auth(&publisher)
                .body(package("Other.Core"))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::CREATED
        );
        let search = json_response(
            c.get(format!("{feed}/v3/search"))
                .bearer_auth(&reader)
                .send()
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(search["totalHits"], 1);
        assert_eq!(search["data"][0]["id"], "Team.Core");
        assert_eq!(
            c.get(format!("{feed}/v3/autocomplete?id=Other.Core"))
                .bearer_auth(&reader)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        let rpc=json_response(c.post(format!("{feed}/mcp")).bearer_auth(&reader).header("accept","application/json, text/event-stream").header("mcp-protocol-version","2025-06-18").json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"get_package","arguments":{"id":"Other.Core"}}})).send().await.unwrap()).await;
        assert!(rpc.get("error").is_some() || rpc["result"]["isError"] == true);
        let file = c
            .get(format!(
                "{feed}/v3/flatcontainer/team.core/1.0.0/team.core.1.0.0.nupkg"
            ))
            .bearer_auth(&publisher)
            .send()
            .await
            .unwrap();
        assert_eq!(file.status(), StatusCode::OK);
        assert_eq!(file.bytes().await.unwrap(), package("Team.Core"));
        c.put(format!("{base}/api/feeds/{}/members/{p}", ids[0]))
            .bearer_auth(ADMIN)
            .json(&json!({"role":"reader"}))
            .send()
            .await
            .unwrap();
        assert_eq!(
            c.put(format!("{feed}/api/v2/package"))
                .bearer_auth(&publisher)
                .body(package("Team.New"))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        c.delete(format!("{base}/api/feeds/{}/members/{p}", ids[0]))
            .bearer_auth(ADMIN)
            .send()
            .await
            .unwrap();
        assert_eq!(
            c.get(format!("{feed}/v3/search"))
                .bearer_auth(&publisher)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            c.get(format!("{base}/feeds/beta/v3/search"))
                .bearer_auth(&publisher)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            c.get(format!("{base}/api/feeds"))
                .send()
                .await
                .unwrap()
                .json::<Value>()
                .await
                .unwrap(),
            json!([])
        );
        c.patch(format!("{base}/api/feeds/{}", ids[1]))
            .bearer_auth(ADMIN)
            .json(&json!({"private":false}))
            .send()
            .await
            .unwrap();
        assert_eq!(
            c.get(format!("{base}/feeds/beta/v3/search"))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        let index =
            json_response(c.get(format!("{base}/v3/index.json")).send().await.unwrap()).await;
        assert!(
            !index["resources"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["@type"] == "VulnerabilityInfo/6.7.0")
        );
        let data = json!({"team.core":[{"severity":2,"versions":"[1.0.0,2.0.0)","url":"https://github.com/advisories/GHSA-example"}]});
        assert_eq!(
            c.post(format!("{base}/api/audit/import"))
                .json(&data)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        let imported = json_response(
            c.post(format!("{base}/api/audit/import"))
                .bearer_auth(ADMIN)
                .json(&data)
                .send()
                .await
                .unwrap(),
        )
        .await;
        let hash = imported["hash"].as_str().unwrap();
        let index = json_response(
            c.get(format!("{base}/feeds/beta/v3/index.json"))
                .send()
                .await
                .unwrap(),
        )
        .await;
        assert!(
            index["resources"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["@type"] == "VulnerabilityInfo/6.7.0")
        );
        let index = json_response(
            c.get(format!("{base}/feeds/beta/v3/vulnerabilities/index.json"))
                .send()
                .await
                .unwrap(),
        )
        .await;
        let page = index[0]["@id"].as_str().unwrap();
        assert!(page.starts_with(&format!("{base}/feeds/beta/")));
        assert_eq!(json_response(c.get(page).send().await.unwrap()).await, data);
        let details = json_response(
            c.get(format!("{base}/feeds/beta/api/packages/team.core"))
                .send()
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(details["versions"][0]["vulnerabilities"][0]["severity"], 2);
        assert_eq!(
            c.get(format!("{feed}/v3/vulnerabilities/index.json"))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            c.post(format!("{base}/api/audit/import"))
                .bearer_auth(ADMIN)
                .json(&json!({"bad":[{"severity":9}]}))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            json_response(
                c.get(format!("{base}/v3/vulnerabilities/{hash}.json"))
                    .send()
                    .await
                    .unwrap()
            )
            .await,
            data
        );
        json_response(
            c.post(format!("{base}/api/audit/import"))
                .bearer_auth(ADMIN)
                .json(&json!([]))
                .send()
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(
            json_response(
                c.get(format!("{base}/v3/vulnerabilities/{hash}.json"))
                    .send()
                    .await
                    .unwrap()
            )
            .await,
            data
        );
        assert_eq!(
            c.get(format!("{base}/api/events"))
                .bearer_auth(&publisher)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        let events = json_response(
            c.get(format!("{base}/api/events?take=200"))
                .bearer_auth(ADMIN)
                .send()
                .await
                .unwrap(),
        )
        .await;
        assert!(
            events
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["type"] == "package.published"
                    && e["target"]["id"] == "Team.Core"
                    && e["status"] == 201)
        );
        assert!(!events.to_string().contains(&publisher));
        assert!(!events.to_string().contains(&reader));
        assert!(!events.to_string().contains(ADMIN));
        let clean = json_response(
            c.get(format!("{base}/feeds/beta/api/packages/team.core"))
                .send()
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(clean["versions"][0]["vulnerabilities"], json!([]));
        assert_eq!(
            json_response(
                c.get(format!("{base}/api/audit/packages/not.in.database"))
                    .send()
                    .await
                    .unwrap()
            )
            .await,
            json!([])
        );
        webhook_checks(&c, &base, &event_state).await;
        trusted_checks(&c, &base, &event_state, &p, &ids[1]).await;
        server.abort();
        db.close().await;
        sqlx::query(&format!("DROP DATABASE {database} WITH (FORCE)"))
            .execute(&root)
            .await
            .unwrap();
    }
}
async fn trusted_checks(c: &Client, base: &str, state: &AppState, profile: &str, feed: &str) {
    use jsonwebtoken::{Algorithm, EncodingKey, Header};
    let jwks: Value = serde_json::from_str(include_str!("fixtures/oidc-jwks.json")).unwrap();
    sqlx::query(
        "UPDATE public.trusted_oidc_keys SET document=$1,fetched_at=now(),last_attempt=now()",
    )
    .bind(jwks)
    .execute(&state.db)
    .await
    .unwrap();
    let policy=json_response(c.post(format!("{base}/api/trusted-publishing")).bearer_auth(ADMIN).json(&json!({"profileId":profile,"name":"Signed CI publisher","repository":"acme/library","repositoryId":"123456","ownerId":"7890","workflow":".github/workflows/publish.yml","gitRef":"refs/heads/main","packagePrefix":"Trusted.","feedId":feed})).send().await.unwrap()).await;
    let now = chrono::Utc::now().timestamp();
    let base_claims = json!({"iss":nushelf::trusted::ISSUER,"aud":base,"sub":"repo:acme@7890/library@123456:ref:refs/heads/main","exp":now+300,"iat":now,"nbf":now-1,"jti":Uuid::new_v4().to_string(),"repository":"acme/library","repository_id":"123456","repository_owner_id":"7890","workflow_ref":"acme/library/.github/workflows/publish.yml@refs/heads/main","ref":"refs/heads/main","event_name":"push"});
    let rsa = EncodingKey::from_rsa_pem(include_bytes!("fixtures/oidc-private.pem")).unwrap();
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some("fixture".into());
    let encode = |claims: &Value| jsonwebtoken::encode(&header, claims, &rsa).unwrap();
    for (field, bad, status) in [
        (
            "aud",
            json!("https://another.example"),
            StatusCode::UNAUTHORIZED,
        ),
        (
            "iss",
            json!("https://untrusted.example"),
            StatusCode::UNAUTHORIZED,
        ),
        ("exp", json!(now - 100), StatusCode::UNAUTHORIZED),
        ("iat", json!(now + 100), StatusCode::UNAUTHORIZED),
        ("repository_id", json!("999999"), StatusCode::FORBIDDEN),
        ("repository_owner_id", json!("123"), StatusCode::FORBIDDEN),
        (
            "workflow_ref",
            json!("acme/library/.github/workflows/other.yml@refs/heads/main"),
            StatusCode::FORBIDDEN,
        ),
        ("ref", json!("refs/heads/other"), StatusCode::FORBIDDEN),
        (
            "event_name",
            json!("pull_request_target"),
            StatusCode::FORBIDDEN,
        ),
        (
            "sub",
            json!("repo:other/library:ref:refs/heads/main"),
            StatusCode::FORBIDDEN,
        ),
    ] {
        let mut claims = base_claims.clone();
        claims[field] = bad;
        claims["jti"] = json!(Uuid::new_v4().to_string());
        assert_eq!(
            c.post(format!("{base}/api/trusted-publishing/exchange"))
                .json(&json!({"token":encode(&claims)}))
                .send()
                .await
                .unwrap()
                .status(),
            status,
            "{field}"
        );
    }
    let token = encode(&base_claims);
    use base64::Engine;
    let parts: Vec<_> = token.split('.').collect();
    let mut altered = base_claims.clone();
    altered["jti"] = json!(Uuid::new_v4().to_string());
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(serde_json::to_vec(&altered).unwrap());
    let tampered = format!("{}.{}.{}", parts[0], payload, parts[2]);
    assert_eq!(
        c.post(format!("{base}/api/trusted-publishing/exchange"))
            .json(&json!({"token":tampered}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let issued = json_response(
        c.post(format!("{base}/api/trusted-publishing/exchange"))
            .json(&json!({"token":token}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        c.post(format!("{base}/api/trusted-publishing/exchange"))
            .json(&json!({"token":token}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    let key = issued["key"].as_str().unwrap();
    let target = format!("{base}/feeds/beta");
    let me = json_response(
        c.get(format!("{target}/api/me"))
            .bearer_auth(key)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(me["scopes"], json!(["package:push", "package:read"]));
    assert_eq!(me["packagePrefix"], "trusted.");
    let expiry =
        chrono::DateTime::parse_from_rfc3339(issued["expiresAt"].as_str().unwrap()).unwrap();
    assert!(expiry.timestamp() - now <= 601);
    assert_eq!(
        c.get(format!("{base}/api/me"))
            .bearer_auth(key)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        c.put(format!("{target}/api/v2/package"))
            .bearer_auth(key)
            .body(package("Outside.Prefix"))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        c.put(format!("{target}/api/v2/package"))
            .bearer_auth(key)
            .body(package("Trusted.Core"))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::CREATED
    );
    assert_eq!(
        c.delete(format!("{target}/api/v2/package/trusted.core/1.0.0"))
            .bearer_auth(key)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    let events = json_response(
        c.get(format!("{base}/api/events?take=200"))
            .bearer_auth(ADMIN)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert!(
        events
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["type"] == "package.published"
                && e["target"]["id"] == "Trusted.Core"
                && e["keyId"] == issued["id"])
    );
    assert!(!events.to_string().contains(key));
    assert!(!events.to_string().contains(&token));
    let mut legacy = base_claims.clone();
    legacy["sub"] = json!("repo:acme/library:ref:refs/heads/main");
    legacy["jti"] = json!(Uuid::new_v4().to_string());
    let legacy = json_response(
        c.post(format!("{base}/api/trusted-publishing/exchange"))
            .json(&json!({"token":encode(&legacy)}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert!(legacy["key"].is_string());
    assert_eq!(
        c.delete(format!(
            "{base}/api/trusted-publishing/{}",
            policy["id"].as_str().unwrap()
        ))
        .bearer_auth(ADMIN)
        .send()
        .await
        .unwrap()
        .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        c.get(format!("{target}/api/me"))
            .bearer_auth(key)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let wrong = jsonwebtoken::encode(
        &Header::new(Algorithm::HS256),
        &base_claims,
        &EncodingKey::from_secret(b"untrusted"),
    )
    .unwrap();
    assert_eq!(
        c.post(format!("{base}/api/trusted-publishing/exchange"))
            .json(&json!({"token":wrong}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
}
async fn webhook_checks(c: &Client, base: &str, state: &AppState) {
    use axum::{Router, extract::State, http::HeaderMap, routing::post};
    type Received = Arc<tokio::sync::Mutex<Vec<(HeaderMap, Bytes)>>>;
    let seen = Arc::new(tokio::sync::Mutex::new(Vec::<(HeaderMap, Bytes)>::new()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/hook", listener.local_addr().unwrap());
    let route = Router::new()
        .route(
            "/hook",
            post(
                |State(seen): State<Received>, headers: HeaderMap, body: Bytes| async move {
                    let mut seen = seen.lock().await;
                    seen.push((headers, body));
                    if seen.len() == 1 {
                        StatusCode::SERVICE_UNAVAILABLE
                    } else {
                        StatusCode::OK
                    }
                },
            ),
        )
        .with_state(seen.clone());
    let receiver = tokio::spawn(async move { axum::serve(listener, route).await.unwrap() });
    let input = json!({"name":"Signed test receiver","url":url,"secretEnv":"WEBHOOK_TEST_SIGNING_KEY","events":["package.published"],"allowHttp":true,"allowPrivateNetwork":true});
    let hook = json_response(
        c.post(format!("{base}/api/webhooks"))
            .bearer_auth(ADMIN)
            .json(&input)
            .send()
            .await
            .unwrap(),
    )
    .await;
    let p = profile(c, base, &format!("Webhook {}", Uuid::new_v4())).await;
    let k = key(c, base, &p, &["package:push"], "").await;
    assert_eq!(
        c.put(format!("{base}/api/v2/package"))
            .bearer_auth(k)
            .body(package("Hook.Sample"))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::CREATED
    );
    assert_eq!(nushelf::events::dispatch(state).await.unwrap(), 1);
    assert_eq!(seen.lock().await.len(), 1);
    let attempt = seen.lock().await;
    let (headers, body) = &attempt[0];
    let timestamp = headers["x-nushelf-timestamp"]
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(
        headers["x-nushelf-signature"].to_str().unwrap(),
        nushelf::events::signature(
            "test-signing-secret-with-at-least-32-characters",
            timestamp,
            body
        )
    );
    let delivery = headers["x-nushelf-delivery"].clone();
    drop(attempt);
    assert_eq!(nushelf::events::dispatch(state).await.unwrap(), 0);
    sqlx::query("UPDATE public.webhook_deliveries SET next_attempt=now()-interval '1 minute' WHERE completed_at IS NULL").execute(&state.db).await.unwrap();
    assert_eq!(nushelf::events::dispatch(state).await.unwrap(), 1);
    assert_eq!(seen.lock().await[1].0["x-nushelf-delivery"], delivery);
    let deliveries = json_response(
        c.get(format!("{base}/api/webhooks/deliveries"))
            .bearer_auth(ADMIN)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(deliveries[0]["attempts"], 2);
    assert_eq!(deliveries[0]["status"], 200);
    assert_eq!(
        c.delete(format!(
            "{base}/api/webhooks/{}",
            hook["id"].as_str().unwrap()
        ))
        .bearer_auth(ADMIN)
        .send()
        .await
        .unwrap()
        .status(),
        StatusCode::NO_CONTENT
    );
    let mut unsafe_hook = input;
    unsafe_hook["allowPrivateNetwork"] = json!(false);
    unsafe_hook["name"] = json!("Blocked receiver");
    json_response(
        c.post(format!("{base}/api/webhooks"))
            .bearer_auth(ADMIN)
            .json(&unsafe_hook)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        c.put(format!("{base}/api/v2/package"))
            .bearer_auth(key(c, base, &p, &["package:push"], "").await)
            .body(package("Hook.Blocked"))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::CREATED
    );
    nushelf::events::dispatch(state).await.unwrap();
    assert_eq!(seen.lock().await.len(), 2);
    receiver.abort();
}
