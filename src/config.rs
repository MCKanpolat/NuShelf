use anyhow::{Context, Result, ensure};
use std::{env, net::SocketAddr};

#[derive(Clone)]
pub struct Config {
    pub database_url: String,
    pub public_url: String,
    pub bind: SocketAddr,
    pub admin_key: String,
    pub max_package_bytes: usize,
    pub database_connections: u32,
    pub storage_provider: String,
    pub storage_path: String,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let public_url = env::var("PUBLIC_URL").unwrap_or_else(|_| "http://localhost:5000".into());
        let parsed = url::Url::parse(&public_url).context("PUBLIC_URL is invalid")?;
        ensure!(
            matches!(parsed.scheme(), "http" | "https") && parsed.host_str().is_some(),
            "PUBLIC_URL must be an HTTP(S) URL"
        );
        ensure!(
            parsed.path() == "/"
                && parsed.query().is_none()
                && parsed.fragment().is_none()
                && parsed.username().is_empty()
                && parsed.password().is_none(),
            "PUBLIC_URL must be an origin without a path, query or credentials"
        );
        let admin_key =
            env::var("ADMIN_API_KEY").context("Set ADMIN_API_KEY (at least 32 characters)")?;
        ensure!(
            admin_key.len() >= 32,
            "ADMIN_API_KEY must contain at least 32 characters"
        );
        let max_package_bytes = env::var("MAX_PACKAGE_BYTES")
            .unwrap_or_else(|_| "104857600".into())
            .parse()?;
        ensure!(
            (1024..=1073741824).contains(&max_package_bytes),
            "MAX_PACKAGE_BYTES must be between 1 KiB and 1 GiB"
        );
        let database_connections = env::var("DATABASE_CONNECTIONS")
            .unwrap_or_else(|_| "20".into())
            .parse()?;
        ensure!(
            (1..=200).contains(&database_connections),
            "DATABASE_CONNECTIONS must be between 1 and 200"
        );
        Ok(Self {
            database_url: env::var("DATABASE_URL").context("Set DATABASE_URL")?,
            public_url: public_url.trim_end_matches('/').into(),
            bind: env::var("BIND_ADDRESS")
                .unwrap_or_else(|_| "127.0.0.1:5000".into())
                .parse()?,
            admin_key,
            max_package_bytes,
            database_connections,
            storage_provider: env::var("STORAGE_PROVIDER").unwrap_or_else(|_| "local".into()),
            storage_path: env::var("STORAGE_PATH").unwrap_or_else(|_| "./data/packages".into()),
        })
    }
}
