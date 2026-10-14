#![cfg(feature = "performance-tests")]
//! Opt-in HTTP performance regression suite. Never uses the application database.
use anyhow::{Context, Result, bail, ensure};
use chrono::Utc;
use nushelf::{AppState, config::Config};
use reqwest::Client;
use serde::Serialize;
use serde_json::{Value, json};
use sqlx::{PgPool, postgres::PgPoolOptions};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::{sync::Barrier, task::JoinHandle};
use uuid::Uuid;

const ADMIN: &str = "performance-only-admin-key-at-least-32-characters";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Settings {
    packages: u32,
    requests_per_case: usize,
    warmups: usize,
    concurrency: Vec<usize>,
    database_connections: u32,
    budget_multiplier: f64,
    report_dir: PathBuf,
}
impl Settings {
    fn from_env() -> Result<Self> {
        let packages = env_number("PERF_PACKAGES", 50_000u32)?;
        let requests_per_case = env_number("PERF_REQUESTS", 100usize)?;
        let warmups = env_number("PERF_WARMUPS", 3usize)?;
        let database_connections = env_number("PERF_DB_CONNECTIONS", 20u32)?;
        let budget_multiplier = env_number("PERF_BUDGET_MULTIPLIER", 1.0f64)?;
        let mut concurrency: Vec<usize> = std::env::var("PERF_CONCURRENCY")
            .unwrap_or_else(|_| "1,8,32".into())
            .split(',')
            .map(|s| s.trim().parse().context("Invalid PERF_CONCURRENCY"))
            .collect::<Result<_>>()?;
        concurrency.sort_unstable();
        concurrency.dedup();
        ensure!(
            (1_000..=200_000).contains(&packages),
            "PERF_PACKAGES must be 1000–200000"
        );
        ensure!(
            (10..=10_000).contains(&requests_per_case),
            "PERF_REQUESTS must be 10–10000"
        );
        ensure!((1..=100).contains(&warmups), "PERF_WARMUPS must be 1–100");
        ensure!(
            !concurrency.is_empty()
                && concurrency
                    .iter()
                    .all(|c| *c > 0 && *c <= 128 && *c <= requests_per_case),
            "Concurrency must be 1–128 and no greater than PERF_REQUESTS"
        );
        ensure!(
            (1..=100).contains(&database_connections),
            "PERF_DB_CONNECTIONS must be 1–100"
        );
        ensure!(
            budget_multiplier.is_finite() && (0.1..=100.0).contains(&budget_multiplier),
            "PERF_BUDGET_MULTIPLIER must be finite and 0.1–100"
        );
        Ok(Self {
            packages,
            requests_per_case,
            warmups,
            concurrency,
            database_connections,
            budget_multiplier,
            report_dir: std::env::var("PERF_REPORT_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| "test-results/performance".into()),
        })
    }
}
fn env_number<T>(name: &str, default: T) -> Result<T>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    match std::env::var(name) {
        Ok(v) => v.parse().map_err(|e| anyhow::anyhow!("{name}: {e}")),
        Err(std::env::VarError::NotPresent) => Ok(default),
        Err(e) => Err(e.into()),
    }
}

#[derive(Clone)]
struct Case {
    name: &'static str,
    query: String,
    sort: &'static str,
    skip: i64,
    prerelease: bool,
    mcp: bool,
    expected_hits: u64,
    first_id: Option<String>,
    first_version: Option<&'static str>,
    version_count: Option<usize>,
    serial_p95_budget_ms: f64,
    min_rps: f64,
}
fn cases(packages: u32) -> Vec<Case> {
    let exact_id = format!("Bench.Package{:06}", packages / 2);
    let prefix = format!("bench.package{:04}", (packages - 100) / 100);
    let full_text_hits = u64::from(packages / 10) + 1;
    let basic = Case {
        name: "exact_id",
        query: exact_id.to_ascii_lowercase(),
        sort: "relevance",
        skip: 0,
        prerelease: false,
        mcp: false,
        expected_hits: 1,
        first_id: Some(exact_id.clone()),
        first_version: Some("1.0.0"),
        version_count: Some(1),
        serial_p95_budget_ms: 500.0,
        min_rps: 10.0,
    };
    vec![
        basic.clone(),
        Case {
            name: "prefix",
            query: prefix,
            expected_hits: 100,
            first_id: None,
            serial_p95_budget_ms: 750.0,
            min_rps: 5.0,
            ..basic.clone()
        },
        Case {
            name: "full_text",
            query: "serialization".into(),
            expected_hits: full_text_hits,
            first_id: None,
            serial_p95_budget_ms: 2000.0,
            min_rps: 2.0,
            ..basic.clone()
        },
        Case {
            name: "fuzzy_fallback",
            query: "perf.serializtion".into(),
            first_id: Some("Perf.Serialization".into()),
            serial_p95_budget_ms: 1000.0,
            min_rps: 5.0,
            ..basic.clone()
        },
        Case {
            name: "no_matches",
            query: "zzzxxyyqq".into(),
            expected_hits: 0,
            first_id: None,
            first_version: None,
            version_count: None,
            serial_p95_budget_ms: 750.0,
            min_rps: 5.0,
            ..basic.clone()
        },
        Case {
            name: "pagination",
            query: "serialization".into(),
            skip: i64::from(packages / 20),
            expected_hits: full_text_hits,
            first_id: None,
            serial_p95_budget_ms: 2000.0,
            min_rps: 2.0,
            ..basic.clone()
        },
        Case {
            name: "browse_recent",
            query: String::new(),
            sort: "recent",
            expected_hits: u64::from(packages),
            first_id: None,
            first_version: None,
            version_count: None,
            serial_p95_budget_ms: 1500.0,
            min_rps: 3.0,
            ..basic.clone()
        },
        Case {
            name: "version_history",
            query: "perf.history".into(),
            first_id: Some("Perf.History".into()),
            first_version: Some("1.0.200"),
            version_count: Some(201),
            serial_p95_budget_ms: 1000.0,
            min_rps: 5.0,
            ..basic.clone()
        },
        Case {
            name: "stable_filter",
            query: "perf.preview".into(),
            first_id: Some("Perf.Preview".into()),
            ..basic.clone()
        },
        Case {
            name: "prerelease_filter",
            query: "perf.preview".into(),
            prerelease: true,
            first_id: Some("Perf.Preview".into()),
            first_version: Some("2.0.0-preview.1"),
            version_count: Some(2),
            ..basic.clone()
        },
        Case {
            name: "mcp_search",
            mcp: true,
            serial_p95_budget_ms: 750.0,
            min_rps: 5.0,
            ..basic
        },
    ]
}

async fn seed(db: &PgPool, packages: u32) -> Result<Uuid> {
    sqlx::migrate!("./migrations").run(db).await?;
    let mut tx = db.begin().await?;
    // Bulk fixture setup has its own budget; timed requests retain the production 15s timeout.
    sqlx::query("SET LOCAL statement_timeout='120s'")
        .execute(&mut *tx)
        .await?;
    let profile = Uuid::new_v4();
    sqlx::query("INSERT INTO profiles(id,name) VALUES($1,'Performance fixtures')")
        .bind(profile)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO packages(id,display_id,owner_id) SELECT CASE n WHEN 1 THEN 'perf.serialization' WHEN 2 THEN 'perf.history' WHEN 3 THEN 'perf.preview' ELSE 'bench.package'||lpad(n::text,6,'0') END, CASE n WHEN 1 THEN 'Perf.Serialization' WHEN 2 THEN 'Perf.History' WHEN 3 THEN 'Perf.Preview' ELSE 'Bench.Package'||lpad(n::text,6,'0') END, $1 FROM generate_series(1,$2) n")
        .bind(profile).bind(packages as i32).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO package_versions(package_id,version,display_version,version_sort,prerelease,semver2,title,description,authors,tags,nuspec,storage_key,size,sha512) SELECT id,'1.0.0','1.0.0','0000000001.0000000000.0000000000.0000000000/1',false,false,display_id,CASE WHEN right(id,1)='0' OR id='perf.serialization' THEN 'Serialization toolkit for JSON documents' ELSE 'Logging toolkit for applications' END,'Performance Team',CASE WHEN right(id,1)='0' OR id='perf.serialization' THEN 'json serialization' ELSE 'logging' END,decode('','hex'),id,1,'' FROM packages").execute(&mut *tx).await?;
    // Long version history and a prerelease/unlisted edge case exercise real response costs.
    sqlx::query("INSERT INTO package_versions(package_id,version,display_version,version_sort,prerelease,semver2,title,description,authors,tags,nuspec,storage_key,size,sha512) SELECT 'perf.history','1.0.'||n,'1.0.'||n,'0000000001.0000000000.'||lpad(n::text,10,'0')||'.0000000000/1',false,false,'Perf.History','Version history fixture','Performance Team','history',decode('','hex'),'perf.history/'||n,1,'' FROM generate_series(1,200) n").execute(&mut *tx).await?;
    for (id, version, listed) in [
        ("perf.preview", "2.0.0-preview.1", true),
        ("perf.history", "0.9.0", false),
    ] {
        let parsed =
            nushelf::version::NuGetVersion::parse(version).map_err(|e| anyhow::anyhow!(e.1))?;
        sqlx::query("INSERT INTO package_versions(package_id,version,display_version,version_sort,prerelease,semver2,listed,title,description,authors,tags,nuspec,storage_key,size,sha512) VALUES($1,$2,$2,$3,$4,$5,$6,$1,'Visibility fixture','Performance Team','visibility',decode('','hex'),$1||'/'||$2,1,'')")
            .bind(id).bind(version).bind(parsed.sort_key).bind(parsed.prerelease).bind(parsed.semver2).bind(listed).execute(&mut *tx).await?;
    }
    sqlx::query("INSERT INTO search_documents(package_id,channel,version,title,description,authors,tags,published_at) SELECT package_id,c.channel,version,title,description,authors,tags,published_at FROM package_versions CROSS JOIN (VALUES(0::smallint),(1::smallint),(2::smallint),(3::smallint)) c(channel) WHERE version='1.0.0'").execute(&mut *tx).await?;
    sqlx::query("SELECT refresh_search('perf.history'), refresh_search('perf.preview')")
        .execute(&mut *tx)
        .await?;
    sqlx::query("ANALYZE").execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(profile)
}

struct RunningServer(JoinHandle<()>);
impl Drop for RunningServer {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Latency {
    p50_ms: f64,
    p95_ms: f64,
    p99_ms: f64,
    max_ms: f64,
}
fn percentile(sorted: &[f64], percentile: usize) -> f64 {
    sorted[(sorted.len() * percentile).div_ceil(100).saturating_sub(1)]
}
fn latency(mut times: Vec<f64>) -> Latency {
    times.sort_by(f64::total_cmp);
    Latency {
        p50_ms: percentile(&times, 50),
        p95_ms: percentile(&times, 95),
        p99_ms: percentile(&times, 99),
        max_ms: *times.last().expect("nonempty measurements"),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Measurement {
    case: String,
    concurrency: usize,
    requests: usize,
    successes: usize,
    errors: usize,
    elapsed_seconds: f64,
    requests_per_second: f64,
    latency: Latency,
    max_p95_ms: f64,
    max_p99_ms: f64,
    min_requests_per_second: f64,
    passed: bool,
    error_examples: Vec<String>,
}

async fn request(client: &Client, base: &str, mcp_key: &str, case: &Case) -> Result<()> {
    let response = if case.mcp {
        client.post(format!("{base}/mcp")).bearer_auth(mcp_key)
            .header("Accept","application/json, text/event-stream").header("MCP-Protocol-Version","2025-06-18")
            .json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"search_packages","arguments":{"query":case.query,"take":20}}})).send().await?
    } else {
        client
            .get(format!("{base}/v3/search"))
            .query(&[
                ("q", case.query.clone()),
                ("sort", case.sort.into()),
                ("skip", case.skip.to_string()),
                ("take", "20".into()),
                ("prerelease", case.prerelease.to_string()),
                ("semVerLevel", "2.0.0".into()),
            ])
            .send()
            .await?
    };
    ensure!(response.status().is_success(), "HTTP {}", response.status());
    // End timing after the complete body is received, decoded and checked.
    let envelope: Value = response.json().await?;
    let payload = if case.mcp {
        ensure!(
            envelope["result"]["isError"] == false,
            "MCP tool returned an error"
        );
        &envelope["result"]["structuredContent"]
    } else {
        &envelope
    };
    ensure!(
        payload["totalHits"].as_u64() == Some(case.expected_hits),
        "Wrong totalHits: expected {}, got {}",
        case.expected_hits,
        payload["totalHits"]
    );
    let data = payload["data"]
        .as_array()
        .context("Search has no data array")?;
    let expected_rows = case.expected_hits.saturating_sub(case.skip as u64).min(20) as usize;
    ensure!(
        data.len() == expected_rows,
        "Expected {expected_rows} results, got {}",
        data.len()
    );
    if let Some(id) = &case.first_id {
        ensure!(data[0]["id"] == id.as_str(), "Wrong first package");
    }
    if let Some(version) = case.first_version {
        ensure!(
            data[0]["version"] == version,
            "Wrong version: {}",
            data[0]["version"]
        );
    }
    if let Some(count) = case.version_count {
        ensure!(
            data[0]["versions"]
                .as_array()
                .is_some_and(|v| v.len() == count),
            "Wrong visible version count"
        );
    }
    Ok(())
}

async fn measure(
    client: &Client,
    base: &str,
    mcp_key: &str,
    case: &Case,
    concurrency: usize,
    settings: &Settings,
) -> Result<Measurement> {
    for _ in 0..settings.warmups {
        request(client, base, mcp_key, case)
            .await
            .with_context(|| format!("{} preflight failed", case.name))?;
    }
    let barrier = Arc::new(Barrier::new(concurrency + 1));
    let next = Arc::new(AtomicUsize::new(0));
    let mut workers = Vec::new();
    for _ in 0..concurrency {
        let (client, base, key, case, barrier, next, count) = (
            client.clone(),
            base.to_owned(),
            mcp_key.to_owned(),
            case.clone(),
            barrier.clone(),
            next.clone(),
            settings.requests_per_case,
        );
        workers.push(tokio::spawn(async move {
            barrier.wait().await;
            let mut samples = Vec::new();
            while next.fetch_add(1, Ordering::Relaxed) < count {
                let start = Instant::now();
                let result = request(&client, &base, &key, &case).await;
                samples.push((
                    start.elapsed().as_secs_f64() * 1000.0,
                    result.err().map(|e| e.to_string()),
                ));
            }
            samples
        }));
    }
    let started = Instant::now();
    barrier.wait().await;
    let mut samples = Vec::new();
    for worker in workers {
        samples.extend(worker.await?);
    }
    let elapsed = started.elapsed().as_secs_f64();
    ensure!(
        samples.len() == settings.requests_per_case,
        "Incomplete load phase"
    );
    let errors = samples.iter().filter(|(_, e)| e.is_some()).count();
    let error_examples = samples
        .iter()
        .filter_map(|(_, e)| e.clone())
        .take(3)
        .collect();
    let latency = latency(samples.into_iter().map(|(time, _)| time).collect());
    let rps = settings.requests_per_case as f64 / elapsed;
    let scale = (concurrency as f64).sqrt() * settings.budget_multiplier;
    let max_p95 = case.serial_p95_budget_ms * scale;
    let max_p99 = max_p95 * 2.0;
    let min_rps = case.min_rps / settings.budget_multiplier;
    let passed =
        errors == 0 && latency.p95_ms <= max_p95 && latency.p99_ms <= max_p99 && rps >= min_rps;
    Ok(Measurement {
        case: case.name.into(),
        concurrency,
        requests: settings.requests_per_case,
        successes: settings.requests_per_case - errors,
        errors,
        elapsed_seconds: elapsed,
        requests_per_second: rps,
        latency,
        max_p95_ms: max_p95,
        max_p99_ms: max_p99,
        min_requests_per_second: min_rps,
        passed,
        error_examples,
    })
}

async fn run(db: &PgPool, settings: &Settings) -> Result<Value> {
    let profile = seed(db, settings.packages).await?;
    let dir = tempfile::tempdir()?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}", listener.local_addr()?);
    let config = Config {
        database_url: String::new(),
        public_url: base.clone(),
        bind: listener.local_addr()?,
        admin_key: ADMIN.into(),
        max_package_bytes: 1024 * 1024,
        database_connections: settings.database_connections,
        storage_provider: "local".into(),
        storage_path: dir.path().to_string_lossy().into_owned(),
    };
    let state = AppState {
        db: db.clone(),
        storage: nushelf::storage::build(&config)?,
        config: Arc::new(config),
        upload_slots: Arc::new(tokio::sync::Semaphore::new(4)),
    };
    let _server = RunningServer(tokio::spawn(async move {
        axum::serve(listener, nushelf::router(state))
            .await
            .expect("performance HTTP server");
    }));
    let client = Client::builder()
        .timeout(Duration::from_secs(30))
        .pool_max_idle_per_host(128)
        .build()?;
    let key_response = client
        .post(format!("{base}/api/profiles/{profile}/keys"))
        .bearer_auth(ADMIN)
        .json(&json!({"label":"Performance MCP","scopes":["mcp:read"]}))
        .send()
        .await?
        .error_for_status()?
        .json::<Value>()
        .await?;
    let mcp_key = key_response["key"].as_str().context("Missing MCP key")?;
    let initialize:Value=client.post(format!("{base}/mcp")).bearer_auth(mcp_key).header("Accept","application/json, text/event-stream")
        .json(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"performance-test","version":"1"}}})).send().await?.error_for_status()?.json().await?;
    ensure!(
        initialize["result"]["protocolVersion"] == "2025-06-18",
        "MCP initialization failed"
    );
    let mut measurements = Vec::new();
    for case in cases(settings.packages) {
        for concurrency in &settings.concurrency {
            let result = measure(&client, &base, mcp_key, &case, *concurrency, settings).await?;
            println!(
                "{} c={}: p50={:.2}ms p95={:.2}ms p99={:.2}ms {:.1}req/s errors={} {}",
                result.case,
                result.concurrency,
                result.latency.p50_ms,
                result.latency.p95_ms,
                result.latency.p99_ms,
                result.requests_per_second,
                result.errors,
                if result.passed { "PASS" } else { "FAIL" }
            );
            measurements.push(result);
        }
    }
    let full_text_plan:Value=sqlx::query_scalar("EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) SELECT count(*) FROM search_documents d WHERE d.channel=2 AND (d.search_vector @@ websearch_to_tsquery('simple','serialization') OR d.package_id LIKE 'serialization%')").fetch_one(db).await?;
    let uses_full_text_index = full_text_plan.to_string().contains("search_fulltext_idx");
    let index_gate_passed = settings.packages < 10_000 || uses_full_text_index;
    let postgres: String = sqlx::query_scalar("SELECT version()").fetch_one(db).await?;
    let mut report = json!({"schemaVersion":1,"generatedAt":Utc::now(),"applicationVersion":env!("CARGO_PKG_VERSION"),"configuration":settings,
        "environment":{"postgres":postgres,"availableCpuThreads":std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1),"operatingSystem":std::env::consts::OS,"architecture":std::env::consts::ARCH},
        "method":{"transport":"real HTTP over container loopback; PostgreSQL in a separate Docker container","loadModel":"closed-loop fixed total requests, barrier-synchronized workers","percentiles":"nearest rank","cache":"warm; preflight per case and concurrency","includes":"HTTP routing, database work, complete JSON body transfer, decoding and correctness checks","excludes":"remote clients, TLS, production object storage and mixed read/write workloads"},
        "dataset":{"packages":settings.packages,"versions":settings.packages+202,"searchDocuments":settings.packages*4,"historyPackageVersions":202,"historyListedVersions":201,"unlistedVersions":1},
        "results":measurements,"queryPlan":{"requiresIndexAtPackages":10000,"usesFullTextIndex":uses_full_text_index,"passed":index_gate_passed,"fullTextCount":full_text_plan}});
    report["passed"] = json!(index_gate_passed && measurements.iter().all(|m| m.passed));
    Ok(report)
}

fn write_reports(settings: &Settings, report: &Value) -> Result<()> {
    std::fs::create_dir_all(&settings.report_dir)?;
    std::fs::write(
        settings.report_dir.join("results.json"),
        serde_json::to_string_pretty(report)?,
    )?;
    let mut md = format!(
        "# NuShelf HTTP performance test results\n\nGenerated: {}\n\nDataset: {} packages, {} versions, {} search documents.\n\nOverall: **{}**. {} requests per case/concurrency, {} warmups, {} database connections.\n\nLatency includes the full HTTP/JSON request and response validation; PostgreSQL runs in a separate Docker container. Warm-cache closed-loop load, nearest-rank percentiles. No production latency guarantee.\n\n| Case | Clients | Requests | p50 ms | p95 ms | p99 ms | Requests/s | Errors | p95 budget ms | Result |\n| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |\n",
        report["generatedAt"].as_str().unwrap_or_default(),
        settings.packages,
        settings.packages + 202,
        settings.packages * 4,
        if report["passed"] == true {
            "PASS"
        } else {
            "FAIL"
        },
        settings.requests_per_case,
        settings.warmups,
        settings.database_connections
    );
    for r in report["results"].as_array().context("Missing results")? {
        md.push_str(&format!(
            "| {} | {} | {} | {:.2} | {:.2} | {:.2} | {:.1} | {} | {:.0} | {} |\n",
            r["case"].as_str().unwrap_or_default(),
            r["concurrency"],
            r["requests"],
            r["latency"]["p50Ms"].as_f64().unwrap_or_default(),
            r["latency"]["p95Ms"].as_f64().unwrap_or_default(),
            r["latency"]["p99Ms"].as_f64().unwrap_or_default(),
            r["requestsPerSecond"].as_f64().unwrap_or_default(),
            r["errors"],
            r["maxP95Ms"].as_f64().unwrap_or_default(),
            if r["passed"] == true { "PASS" } else { "FAIL" }
        ));
    }
    md.push_str(&format!("\nFull-text index used: **{}**. See `results.json` for configuration, effective p99/throughput budgets, error samples, and the PostgreSQL execution plan.\n",report["queryPlan"]["usesFullTextIndex"]));
    std::fs::write(settings.report_dir.join("results.md"), md)?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn http_performance_regression() -> Result<()> {
    let settings = Settings::from_env()?;
    // Do not leave an earlier PASS report behind if setup or preflight fails.
    std::fs::create_dir_all(&settings.report_dir)?;
    std::fs::write(
        settings.report_dir.join("results.json"),
        serde_json::to_string_pretty(
            &json!({"schemaVersion":1,"passed":false,"phase":"running","generatedAt":Utc::now(),"configuration":settings}),
        )?,
    )?;
    std::fs::write(
        settings.report_dir.join("results.md"),
        "# NuShelf HTTP performance test results\n\nStatus: **RUNNING**. Measurements are not complete.\n",
    )?;
    let connection = std::env::var("TEST_DATABASE_URL")
        .context("Run with compose.test.yml --profile performance")?;
    let mut url = url::Url::parse(&connection)?;
    ensure!(
        url.path() == "/nushelf_test",
        "Performance tests require the isolated nushelf_test database connection"
    );
    let root = PgPool::connect(&connection).await?;
    let name = format!("nushelf_perf_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE DATABASE {name}"))
        .execute(&root)
        .await?;
    url.set_path(&format!("/{name}"));
    let outcome = async {
        let db = PgPoolOptions::new()
            .max_connections(settings.database_connections)
            .acquire_timeout(Duration::from_secs(10))
            .after_connect(|conn, _| {
                Box::pin(async move {
                    sqlx::query("SET statement_timeout='15s'")
                        .execute(&mut *conn)
                        .await?;
                    sqlx::query("SET idle_in_transaction_session_timeout='90s'")
                        .execute(&mut *conn)
                        .await?;
                    Ok(())
                })
            })
            .connect(url.as_str())
            .await?;
        let result = run(&db, &settings).await;
        db.close().await;
        result
    }
    .await;
    // Also clean up ordinary failures, including failed latency budgets and preflight.
    let cleanup = sqlx::query(&format!("DROP DATABASE {name} WITH (FORCE)"))
        .execute(&root)
        .await;
    root.close().await;
    if let Err(error) = cleanup {
        return Err(error.into());
    }
    let report = match outcome {
        Ok(report) => report,
        Err(error) => {
            std::fs::write(
                settings.report_dir.join("results.json"),
                serde_json::to_string_pretty(
                    &json!({"schemaVersion":1,"passed":false,"phase":"setup_or_preflight_failed","generatedAt":Utc::now(),"configuration":settings,"error":error.to_string()}),
                )?,
            )?;
            std::fs::write(
                settings.report_dir.join("results.md"),
                format!(
                    "# NuShelf HTTP performance test results\n\nStatus: **FAIL**. Setup or preflight did not complete.\n\n{}\n",
                    error
                ),
            )?;
            return Err(error);
        }
    };
    write_reports(&settings, &report)?;
    println!("Reports written to {}", settings.report_dir.display());
    if report["passed"] != true {
        bail!("Performance regression gates failed; see results.json/results.md");
    }
    Ok(())
}

#[test]
fn nearest_rank_percentiles_are_correct() {
    let values: Vec<f64> = (1..=100).rev().map(f64::from).collect();
    let summary = latency(values);
    assert_eq!(
        (
            summary.p50_ms,
            summary.p95_ms,
            summary.p99_ms,
            summary.max_ms
        ),
        (50.0, 95.0, 99.0, 100.0)
    );
    let singleton = latency(vec![7.0]);
    assert_eq!(
        (singleton.p50_ms, singleton.p95_ms, singleton.p99_ms),
        (7.0, 7.0, 7.0)
    );
}

#[tokio::test]
async fn load_phase_rejects_slow_successful_responses() -> Result<()> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}", listener.local_addr()?);
    let app=axum::Router::new().route("/v3/search",axum::routing::get(|| async {
        tokio::time::sleep(Duration::from_millis(20)).await;
        axum::Json(json!({"totalHits":1,"data":[{"id":"Bench.Package025000","version":"1.0.0","versions":[{"version":"1.0.0"}]}]}))
    }));
    let _server = RunningServer(tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("slow fixture server");
    }));
    let settings = Settings {
        packages: 50_000,
        requests_per_case: 10,
        warmups: 1,
        concurrency: vec![2],
        database_connections: 20,
        budget_multiplier: 1.0,
        report_dir: PathBuf::new(),
    };
    let mut case = cases(settings.packages).remove(0);
    case.serial_p95_budget_ms = 1.0;
    let measurement = measure(&Client::new(), &base, "", &case, 2, &settings).await?;
    ensure!(
        measurement.errors == 0 && measurement.successes == 10,
        "Expected successful, valid HTTP responses"
    );
    ensure!(
        measurement.latency.p95_ms > measurement.max_p95_ms && !measurement.passed,
        "Slow HTTP responses must fail the latency gate"
    );
    Ok(())
}

#[tokio::test]
async fn load_phase_rejects_fast_incorrect_responses() -> Result<()> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}", listener.local_addr()?);
    let app = axum::Router::new().route(
        "/v3/search",
        axum::routing::get(|| async { axum::Json(json!({"totalHits":0,"data":[]})) }),
    );
    let _server = RunningServer(tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("incorrect fixture server");
    }));
    let settings = Settings {
        packages: 50_000,
        requests_per_case: 10,
        warmups: 1,
        concurrency: vec![1],
        database_connections: 20,
        budget_multiplier: 1.0,
        report_dir: PathBuf::new(),
    };
    let case = cases(settings.packages).remove(0);
    let result = measure(&Client::new(), &base, "", &case, 1, &settings).await;
    ensure!(
        result.is_err(),
        "HTTP 200 alone must not pass correctness checks"
    );
    ensure!(
        format!("{:#}", result.err().expect("preflight error")).contains("Wrong totalHits"),
        "The failure must identify the invalid search response"
    );
    Ok(())
}
