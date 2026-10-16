use anyhow::{Context, Result, ensure};
use std::{env, net::SocketAddr, time::Duration};

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
    /// Separate listener for `GET /metrics`; metrics are off when unset.
    pub metrics_bind: Option<SocketAddr>,
    pub limits: Limits,
}

/// Abuse-protection and capacity knobs. Defaults suit a single small deployment.
#[derive(Clone, Debug)]
pub struct Limits {
    /// Package uploads processed at once; further uploads receive 429.
    pub concurrent_uploads: usize,
    /// Rejected credentials tolerated per client address and window; 0 disables the limiter.
    pub auth_failures: u32,
    pub auth_window: Duration,
    /// Honor `X-Forwarded-For` for the client address. Enable only behind a proxy that sets it.
    pub trust_proxy_headers: bool,
    /// Time a handler may take to produce response headers.
    pub request_timeout: Duration,
    /// Same bound for package uploads, which also covers reading the request body.
    pub upload_timeout: Duration,
    /// Longest silence tolerated while reading a request body or streaming a response.
    pub body_idle_timeout: Duration,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            concurrent_uploads: 4,
            auth_failures: 20,
            auth_window: Duration::from_secs(300),
            trust_proxy_headers: false,
            request_timeout: Duration::from_secs(120),
            upload_timeout: Duration::from_secs(900),
            body_idle_timeout: Duration::from_secs(30),
        }
    }
}
impl Limits {
    fn from_env() -> Result<Self> {
        let default = Self::default();
        let number = |name: &str, default: u64, range: std::ops::RangeInclusive<u64>| {
            let value = match env::var(name) {
                Ok(v) if !v.is_empty() => {
                    v.parse().with_context(|| format!("{name} is invalid"))?
                }
                _ => default,
            };
            ensure!(
                range.contains(&value),
                "{name} must be between {} and {}",
                range.start(),
                range.end()
            );
            Ok::<_, anyhow::Error>(value)
        };
        let trust_proxy_headers = match env::var("TRUST_PROXY_HEADERS").as_deref() {
            Ok("true" | "1") => true,
            Ok("false" | "0" | "") | Err(_) => false,
            Ok(_) => anyhow::bail!("TRUST_PROXY_HEADERS must be true or false"),
        };
        Ok(Self {
            concurrent_uploads: number("MAX_CONCURRENT_UPLOADS", 4, 1..=64)? as usize,
            auth_failures: number(
                "AUTH_FAILURE_LIMIT",
                default.auth_failures.into(),
                0..=100_000,
            )? as u32,
            auth_window: Duration::from_secs(number(
                "AUTH_FAILURE_WINDOW_SECONDS",
                default.auth_window.as_secs(),
                10..=86_400,
            )?),
            trust_proxy_headers,
            request_timeout: Duration::from_secs(number(
                "REQUEST_TIMEOUT_SECONDS",
                default.request_timeout.as_secs(),
                5..=3600,
            )?),
            upload_timeout: Duration::from_secs(number(
                "UPLOAD_TIMEOUT_SECONDS",
                default.upload_timeout.as_secs(),
                10..=7200,
            )?),
            body_idle_timeout: Duration::from_secs(number(
                "BODY_IDLE_TIMEOUT_SECONDS",
                default.body_idle_timeout.as_secs(),
                5..=600,
            )?),
        })
    }
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
        let bind: SocketAddr = env::var("BIND_ADDRESS")
            .unwrap_or_else(|_| "127.0.0.1:5000".into())
            .parse()?;
        let metrics_bind = match env::var("METRICS_BIND_ADDRESS") {
            Ok(value) if !value.is_empty() => Some(
                value
                    .parse::<SocketAddr>()
                    .context("METRICS_BIND_ADDRESS is invalid")?,
            ),
            _ => None,
        };
        ensure!(
            metrics_bind.is_none_or(|metrics| metrics.port() != bind.port()),
            "METRICS_BIND_ADDRESS must use a different port from BIND_ADDRESS"
        );
        Ok(Self {
            database_url: env::var("DATABASE_URL").context("Set DATABASE_URL")?,
            public_url: public_url.trim_end_matches('/').into(),
            bind,
            admin_key,
            max_package_bytes,
            database_connections,
            storage_provider: env::var("STORAGE_PROVIDER").unwrap_or_else(|_| "local".into()),
            storage_path: env::var("STORAGE_PATH").unwrap_or_else(|_| "./data/packages".into()),
            metrics_bind,
            limits: Limits::from_env()?,
        })
    }
}
