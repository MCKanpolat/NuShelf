//! Opt-in synthetic benchmark. All data lives in a new, disposable test database.
use anyhow::Result;
use nushelf::{
    AppState,
    config::Config,
    registry::{SearchQuery, search_value},
};
use serde_json::json;
use sqlx::PgPool;
use std::{sync::Arc, time::Instant};
use uuid::Uuid;

#[tokio::main]
async fn main() -> Result<()> {
    let connection = std::env::var("TEST_DATABASE_URL")?;
    let root = PgPool::connect(&connection).await?;
    let db_name = format!("nushelf_bench_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE DATABASE {db_name}"))
        .execute(&root)
        .await?;
    let mut url = url::Url::parse(&connection)?;
    url.set_path(&format!("/{db_name}"));
    let db = PgPool::connect(url.as_str()).await?;
    sqlx::migrate!("./migrations").run(&db).await?;
    let profile = Uuid::new_v4();
    sqlx::query("INSERT INTO profiles(id,name) VALUES($1,'Synthetic benchmark')")
        .bind(profile)
        .execute(&db)
        .await?;
    sqlx::query("INSERT INTO packages(id,display_id,owner_id) SELECT 'bench.package'||lpad(n::text,6,'0'), 'Bench.Package'||lpad(n::text,6,'0'), $1 FROM generate_series(1,50000) n").bind(profile).execute(&db).await?;
    sqlx::query("INSERT INTO package_versions(package_id,version,display_version,version_sort,prerelease,semver2,title,description,authors,tags,nuspec,storage_key,size,sha512) SELECT id,'1.0.0','1.0.0','0000000001.0000000000.0000000000.0000000000/1',false,false,display_id,CASE WHEN right(id,1)='0' THEN 'Serialization toolkit for JSON documents' ELSE 'Logging toolkit for applications' END,'Benchmark Team',CASE WHEN right(id,1)='0' THEN 'json serialization' ELSE 'logging' END,decode('','hex'),id,1,'' FROM packages").execute(&db).await?;
    sqlx::query("INSERT INTO search_documents(package_id,channel,version,title,description,authors,tags,published_at) SELECT package_id,c.channel,version,title,description,authors,tags,published_at FROM package_versions CROSS JOIN (VALUES(0::smallint),(1::smallint),(2::smallint),(3::smallint)) c(channel)").execute(&db).await?;
    sqlx::query("ANALYZE").execute(&db).await?;
    let dir = tempfile::tempdir()?;
    let config = Config {
        database_url: url.to_string(),
        public_url: "http://benchmark:5000".into(),
        bind: "127.0.0.1:5000".parse()?,
        admin_key: "benchmark-only-admin-key-at-least-32".into(),
        max_package_bytes: 1024 * 1024,
        database_connections: 20,
        storage_provider: "local".into(),
        storage_path: dir.path().to_string_lossy().into_owned(),
    };
    let state = AppState {
        db: db.clone(),
        storage: nushelf::storage::build(&config)?,
        config: Arc::new(config),
        upload_slots: Arc::new(tokio::sync::Semaphore::new(4)),
        feeds: Arc::default(),
        feed: None,
        read_prefix: String::new(),
    };
    let mut results = Vec::new();
    for (label, q, sort) in [
        ("exact ID", "bench.package025000", "relevance"),
        ("full text", "serialization", "relevance"),
        ("prefix", "bench.package0499", "relevance"),
        ("browse recent", "", "recent"),
    ] {
        let query = SearchQuery {
            q: q.into(),
            take: Some(20),
            sem_ver_level: Some("2.0.0".into()),
            sort: Some(sort.into()),
            ..Default::default()
        };
        for _ in 0..3 {
            search_value(&state, &query)
                .await
                .map_err(|e| anyhow::anyhow!("{}", e.1))?;
        }
        let mut times = Vec::new();
        let mut hits = 0;
        for _ in 0..30 {
            let start = Instant::now();
            let value = search_value(&state, &query)
                .await
                .map_err(|e| anyhow::anyhow!("{}", e.1))?;
            times.push(start.elapsed().as_secs_f64() * 1000.0);
            hits = value["totalHits"].as_i64().unwrap_or(0);
        }
        times.sort_by(f64::total_cmp);
        results.push(json!({"case":label,"query":q,"hits":hits,"medianMs":(times[15]*100.0).round()/100.0,"p95Ms":(times[28]*100.0).round()/100.0}));
    }
    let plan: Vec<String>=sqlx::query_scalar("EXPLAIN (ANALYZE, BUFFERS) SELECT count(*) FROM search_documents d WHERE d.channel=2 AND (d.search_vector @@ websearch_to_tsquery('simple','serialization') OR d.package_id LIKE 'serialization%')").fetch_all(&db).await?;
    println!(
        "{}",
        serde_json::to_string_pretty(
            &json!({"packages":50000,"searchDocuments":200000,"samplesPerCase":30,"warmups":3,"scope":"service-layer search including PostgreSQL count, pagination and version list; excludes external HTTP latency; synthetic data, one caller","results":results,"fullTextPlan":plan})
        )?
    );
    db.close().await;
    sqlx::query(&format!("DROP DATABASE {db_name} WITH (FORCE)"))
        .execute(&root)
        .await?;
    root.close().await;
    Ok(())
}
