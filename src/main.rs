use anyhow::Result;
use nushelf::{AppState, config::Config, router, storage};
use sqlx::postgres::PgPoolOptions;
use std::{sync::Arc, time::Duration};

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "nushelf=info,tower_http=info".into()),
        )
        .init();
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
    sqlx::migrate!("./migrations").run(&db).await?;
    let storage = storage::build(&config)?;
    let address = config.bind;
    let state = AppState {
        db: db.clone(),
        storage,
        config: Arc::new(config),
        upload_slots: Arc::new(tokio::sync::Semaphore::new(4)),
    };
    let listener = tokio::net::TcpListener::bind(address).await?;
    tracing::info!(%address,"NuShelf is ready");
    axum::serve(listener, router(state))
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
