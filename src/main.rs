use anyhow::Result;
use nushelf::{AppState, backup, config::Config, router, storage};
use sqlx::postgres::PgPoolOptions;
use std::{sync::Arc, time::Duration};

const USAGE: &str = "Usage: nushelf [serve]\n       nushelf storage-export <dir>   copy package storage into <dir>\n       nushelf storage-import <dir>   restore package storage from <dir>\n       nushelf storage-verify <dir>   check <dir> against its manifest";

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "nushelf=info,tower_http=info".into()),
        )
        .init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        [] | ["serve"] => {}
        ["storage-verify", dir] => {
            let files = backup::verify(dir.as_ref()).await?;
            println!("Backup is intact: {files} objects verified");
            return Ok(());
        }
        [command @ ("storage-export" | "storage-import"), dir] => {
            let store = storage::build(&Config::from_env()?)?;
            let summary = if command == "storage-export" {
                backup::export(store, dir.as_ref()).await?
            } else {
                backup::import(store, dir.as_ref()).await?
            };
            println!(
                "{} objects copied ({} bytes), {} already up to date",
                summary.copied, summary.bytes, summary.skipped
            );
            return Ok(());
        }
        ["--help" | "-h" | "help"] => {
            println!("{USAGE}");
            return Ok(());
        }
        _ => anyhow::bail!("{USAGE}"),
    }
    let config = Config::from_env()?;
    let db = PgPoolOptions::new()
        .max_connections(config.database_connections)
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
        .connect(&config.database_url)
        .await?;
    if let Some(address) = config.metrics_bind {
        let handle = nushelf::telemetry::install()?;
        let metrics_listener = tokio::net::TcpListener::bind(address).await?;
        let db = db.clone();
        tokio::spawn(async move {
            if let Err(e) = nushelf::telemetry::serve(metrics_listener, handle, db).await {
                tracing::error!(error = %e, "metrics endpoint stopped");
            }
        });
    }
    sqlx::migrate!("./migrations").run(&db).await?;
    let storage = storage::build(&config)?;
    let address = config.bind;
    let upload_slots = config.limits.concurrent_uploads;
    let state = AppState {
        db: db.clone(),
        storage,
        config: Arc::new(config),
        upload_slots: Arc::new(tokio::sync::Semaphore::new(upload_slots)),
        guard: Arc::default(),
        feeds: Arc::default(),
        feed: None,
        read_prefix: String::new(),
    };
    nushelf::feeds::initialize(&state)
        .await
        .map_err(|e| anyhow::anyhow!(e.1))?;
    let listener = tokio::net::TcpListener::bind(address).await?;
    nushelf::audit::worker(state.clone());
    nushelf::maintenance::worker(state.clone());
    nushelf::webhooks::worker(state.clone());
    nushelf::upstream::mirror::worker(state.clone());
    tracing::info!(%address,"NuShelf is ready");
    axum::serve(
        listener,
        router(state).into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown())
    .await?;
    db.close().await;
    Ok(())
}
async fn shutdown() {
    let ctrl_c = async {
        tokio::signal::ctrl_c().await.expect("Ctrl+C handler");
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("terminate handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! { _ = ctrl_c => {}, _ = terminate => {} }
}
