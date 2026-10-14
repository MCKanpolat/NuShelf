use super::sources::Source;
use crate::error::{ApiError, ApiResult};
use axum::{body::Bytes, http::StatusCode};
use chrono::{Duration, Utc};
use reqwest::redirect::Policy;
use serde_json::Value;
use std::net::{IpAddr, SocketAddr};
pub const METADATA_LIMIT: usize = 16 * 1024 * 1024;
pub fn failure(message: &str) -> ApiError {
    ApiError(StatusCode::BAD_GATEWAY, message.into())
}
fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let o = ip.octets();
            !ip.is_private()
                && !ip.is_loopback()
                && !ip.is_link_local()
                && !ip.is_unspecified()
                && !ip.is_broadcast()
                && !ip.is_multicast()
                && o[0] != 0
                && o[0] < 224
                && !(o[0] == 100 && (64..=127).contains(&o[1]))
                && !(o[0] == 198 && (o[1] == 18 || o[1] == 19))
                && !(o[0] == 192 && o[1] == 0)
                && !(o[0] == 198 && o[1] == 51 && o[2] == 100)
                && !(o[0] == 203 && o[1] == 0 && o[2] == 113)
        }
        IpAddr::V6(ip) => ip.to_ipv4().map_or_else(
            || {
                !ip.is_loopback()
                    && !ip.is_unspecified()
                    && !ip.is_multicast()
                    && !ip.is_unique_local()
                    && !ip.is_unicast_link_local()
                    && ip.segments()[0] & 0xe000 == 0x2000
                    && !(ip.segments()[0] == 0x2001 && ip.segments()[1] == 0x0db8)
            },
            |v| public_ip(IpAddr::V4(v)),
        ),
    }
}
async fn client(source: &Source, url: &url::Url) -> ApiResult<reqwest::Client> {
    if !matches!(url.scheme(), "https" | "http")
        || (url.scheme() == "http" && !source.allow_http)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(failure("Upstream returned an unsafe URL"));
    }
    let host = match url
        .host()
        .ok_or_else(|| failure("Upstream URL has no host"))?
    {
        url::Host::Domain(name) => name.to_owned(),
        url::Host::Ipv4(ip) => ip.to_string(),
        url::Host::Ipv6(ip) => ip.to_string(),
    };
    let port = url
        .port_or_known_default()
        .ok_or_else(|| failure("Invalid upstream port"))?;
    let addresses: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), port))
        .await
        .map_err(|_| failure("Upstream DNS lookup failed"))?
        .collect();
    if addresses.is_empty()
        || (!source.allow_private_network && addresses.iter().any(|a| !public_ip(a.ip())))
    {
        return Err(failure(
            "Private/reserved upstream addresses require explicit administrator permission",
        ));
    }
    // Pin validated DNS addresses for this request; do not resolve them again when connecting.
    reqwest::Client::builder()
        .no_proxy()
        .user_agent(concat!("NuShelf/", env!("CARGO_PKG_VERSION")))
        .redirect(Policy::none())
        .resolve_to_addrs(&host, &addresses)
        .timeout(std::time::Duration::from_secs(
            source.timeout_seconds as u64,
        ))
        .connect_timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|_| failure("Cannot create upstream HTTP client"))
}
pub async fn bytes(source: &Source, target: &str, limit: usize) -> ApiResult<Option<Bytes>> {
    tokio::time::timeout(
        std::time::Duration::from_secs(source.timeout_seconds as u64),
        fetch(source, target, limit),
    )
    .await
    .map_err(|_| {
        ApiError(
            StatusCode::GATEWAY_TIMEOUT,
            "Upstream request timed out".into(),
        )
    })?
}
async fn fetch(source: &Source, target: &str, limit: usize) -> ApiResult<Option<Bytes>> {
    let origin = url::Url::parse(&source.url)
        .map_err(|_| failure("Invalid upstream origin"))?
        .origin();
    let mut url = url::Url::parse(target).map_err(|_| failure("Invalid upstream resource URL"))?;
    for _ in 0..5 {
        let client = client(source, &url).await?;
        let mut request = client.get(url.clone());
        if source.auth_type != "none" && url.origin() == origin {
            let secret = source
                .credential_env
                .as_ref()
                .and_then(|n| std::env::var(n).ok())
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    failure("Upstream credential environment variable is not configured")
                })?;
            request = if source.auth_type == "basic" {
                request.basic_auth(source.username.as_deref().unwrap_or_default(), Some(secret))
            } else {
                reqwest::header::HeaderValue::from_str(&format!("Bearer {secret}"))
                    .map_err(|_| failure("Invalid upstream authentication header"))?;
                request.bearer_auth(secret)
            };
        }
        let mut response = request
            .send()
            .await
            .map_err(|_| failure("Upstream connection failed"))?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if response.status().is_redirection() {
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|h| h.to_str().ok())
                .ok_or_else(|| failure("Upstream redirect has no Location"))?;
            url = url
                .join(location)
                .map_err(|_| failure("Invalid upstream redirect"))?;
            continue;
        }
        if !response.status().is_success() {
            return Err(ApiError(
                if response.status().is_server_error() {
                    StatusCode::SERVICE_UNAVAILABLE
                } else {
                    StatusCode::BAD_GATEWAY
                },
                format!("Upstream returned HTTP {}", response.status().as_u16()),
            ));
        }
        if response.content_length().is_some_and(|n| n > limit as u64) {
            return Err(failure(
                "Upstream response exceeds the configured size limit",
            ));
        }
        let mut result = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| failure("Incomplete upstream response"))?
        {
            if result.len().saturating_add(chunk.len()) > limit {
                return Err(failure(
                    "Upstream response exceeds the configured size limit",
                ));
            }
            result.extend_from_slice(&chunk);
        }
        return Ok(Some(Bytes::from(result)));
    }
    Err(failure("Too many upstream redirects"))
}
pub async fn json(source: &Source, target: &str) -> ApiResult<Option<Value>> {
    bytes(source, target, METADATA_LIMIT)
        .await?
        .map(|data| serde_json::from_slice(&data).map_err(|_| failure("Invalid upstream JSON")))
        .transpose()
}
pub fn resource(index: &Value, kind: &str) -> ApiResult<String> {
    let resources = index["resources"]
        .as_array()
        .ok_or_else(|| failure("Invalid NuGet service index"))?;
    let preferred = resources
        .iter()
        .find(|r| r["@type"].as_str() == Some(&format!("{kind}/3.6.0")))
        .or_else(|| {
            resources.iter().find(|r| {
                r["@type"]
                    .as_str()
                    .is_some_and(|t| t == kind || t.starts_with(&format!("{kind}/")))
            })
        });
    let value = preferred
        .and_then(|r| r["@id"].as_str())
        .ok_or_else(|| failure("Upstream is missing a required NuGet resource"))?;
    let url = url::Url::parse(value).map_err(|_| failure("Invalid service index resource"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(failure("Unsupported upstream resource URL"));
    }
    Ok(format!("{}/", value.trim_end_matches('/')))
}
pub fn resource_for(state: &crate::AppState, index: &Value, kind: &str) -> ApiResult<String> {
    let value = resource(index, kind)?;
    let url = url::Url::parse(&value).map_err(|_| failure("Invalid upstream resource"))?;
    let own = url::Url::parse(&state.config.public_url).map_err(ApiError::internal)?;
    let path = if kind == "PackageBaseAddress" {
        "/v3/flatcontainer/"
    } else {
        "/v3/registration/"
    };
    if url.origin() == own.origin() && url.path() == path {
        return Err(failure("Upstream resource points back to this registry"));
    }
    Ok(value)
}
pub async fn fetch_index(source: &Source) -> ApiResult<Value> {
    let value = json(source, &source.url)
        .await?
        .ok_or_else(|| failure("NuGet service index was not found"))?;
    resource(&value, "PackageBaseAddress")?;
    resource(&value, "RegistrationsBaseUrl")?;
    Ok(value)
}
pub fn transient(error: &ApiError) -> bool {
    matches!(
        error.0,
        StatusCode::SERVICE_UNAVAILABLE | StatusCode::GATEWAY_TIMEOUT
    ) || (error.0 == StatusCode::BAD_GATEWAY
        && [
            "Upstream connection failed",
            "Upstream DNS lookup failed",
            "Incomplete upstream response",
        ]
        .contains(&error.1.as_str()))
}
pub async fn index(source: &Source) -> ApiResult<Value> {
    if let Some(index) = &source.service_index
        && source
            .index_checked_at
            .is_some_and(|t| t + Duration::seconds(source.metadata_ttl_seconds as i64) > Utc::now())
    {
        return Ok(index.clone());
    }
    match fetch_index(source).await {
        Ok(value) => Ok(value),
        Err(e) if source.stale_on_error && transient(&e) && source.service_index.is_some() => {
            Ok(source.service_index.clone().unwrap())
        }
        Err(e) => Err(e),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn blocks_private_addresses() {
        for ip in [
            "127.0.0.1",
            "10.0.0.1",
            "169.254.169.254",
            "100.64.0.1",
            "::1",
            "::ffff:127.0.0.1",
        ] {
            assert!(!public_ip(ip.parse().unwrap()));
        }
        assert!(public_ip("1.1.1.1".parse().unwrap()));
    }
}
