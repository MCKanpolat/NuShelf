#![allow(dead_code)]
use nushelf::{AppState, config::Config};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::{
    io::{Cursor, Write},
    sync::Arc,
};
use uuid::Uuid;
pub const ADMIN: &str = "protocol-tests-admin-key-at-least-32-characters";
pub struct Server {
    pub state: AppState,
    pub base: String,
    pub client: reqwest::Client,
    pub key: String,
    task: tokio::task::JoinHandle<()>,
    root: PgPool,
    name: String,
    _dir: tempfile::TempDir,
}
impl Server {
    pub async fn start() -> Self {
        let _ = tracing_subscriber::fmt()
            .with_env_filter("nushelf=debug")
            .try_init();
        let database = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
        let root = PgPool::connect(&database).await.unwrap();
        let name = format!("protocol_{}", Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE DATABASE {name}"))
            .execute(&root)
            .await
            .unwrap();
        let mut url = url::Url::parse(&database).unwrap();
        url.set_path(&format!("/{name}"));
        let db = PgPool::connect(url.as_str()).await.unwrap();
        sqlx::migrate!("./migrations").run(&db).await.unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let dir = tempfile::tempdir().unwrap();
        let config = Config {
            database_url: url.to_string(),
            public_url: base.clone(),
            bind: listener.local_addr().unwrap(),
            admin_key: ADMIN.into(),
            max_package_bytes: 2 * 1024 * 1024,
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
        let app = nushelf::router(state.clone());
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = reqwest::Client::new();
        let p = response(
            client
                .post(format!("{base}/api/profiles"))
                .bearer_auth(ADMIN)
                .json(&json!({"name":"Protocol publisher"}))
                .send()
                .await
                .unwrap(),
        )
        .await;
        let key=response(client.post(format!("{base}/api/profiles/{}/keys",p["id"].as_str().unwrap())).bearer_auth(ADMIN).json(&json!({"label":"protocol","scopes":["package:push","package:unlist","package:read","package:deprecate"]})).send().await.unwrap()).await["key"].as_str().unwrap().to_string();
        Self {
            state,
            base,
            client,
            key,
            task,
            root,
            name,
            _dir: dir,
        }
    }
    pub async fn push(&self, id: &str, version: &str) {
        let r = self
            .client
            .put(format!("{}/api/v2/package", self.base))
            .header("X-NuGet-ApiKey", &self.key)
            .body(package(id, version))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 201, "{}", r.text().await.unwrap());
    }
    pub async fn stop(self) {
        self.task.abort();
        for pool in self.state.feeds.0.lock().await.values() {
            pool.close().await;
        }
        self.state.db.close().await;
        sqlx::query(&format!("DROP DATABASE {} WITH (FORCE)", self.name))
            .execute(&self.root)
            .await
            .unwrap();
        self.root.close().await;
    }
}
pub fn package(id: &str, version: &str) -> Vec<u8> {
    let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
    w.start_file("package.nuspec", zip::write::SimpleFileOptions::default())
        .unwrap();
    write!(w,"<package><metadata><id>{id}</id><version>{version}</version><authors>Team</authors><description>JSON parser &amp; helpers</description><tags>json</tags></metadata></package>").unwrap();
    w.finish().unwrap().into_inner()
}
pub async fn response(r: reqwest::Response) -> Value {
    let status = r.status();
    let text = r.text().await.unwrap();
    assert!(status.is_success(), "{status}: {text}");
    serde_json::from_str(&text).unwrap()
}
pub async fn xml(r: reqwest::Response) -> String {
    let status = r.status();
    let text = r.text().await.unwrap();
    assert!(status.is_success(), "{status}: {text}");
    roxmltree::Document::parse(&text).unwrap();
    text
}
pub fn properties(s: &str, name: &str) -> Vec<String> {
    let d = roxmltree::Document::parse(s).unwrap();
    d.descendants()
        .filter(|n| n.tag_name().name() == name)
        .filter_map(|n| n.text().map(str::to_string))
        .collect()
}
