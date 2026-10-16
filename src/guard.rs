//! Process-local operational guards: a per-client limiter for rejected
//! credentials and the cache of the storage write probe used by readiness.
//!
//! State is deliberately in memory. Replicas each enforce their own budget, so
//! the effective limit scales with replica count; that is enough to blunt
//! online key guessing without adding a shared dependency.
use crate::{AppState, config::Limits};
use axum::{
    extract::{ConnectInfo, Request},
    http::{HeaderMap, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::Mutex,
    time::{Duration, Instant},
};

const AUTH_BLOCKED: &str = "nushelf_auth_rate_limited_total";
/// Upper bound on tracked client addresses, so spoofed sources cannot grow memory.
const MAX_TRACKED_CLIENTS: usize = 50_000;

#[derive(Default)]
pub struct Guard {
    failures: Mutex<HashMap<IpAddr, (u32, Instant)>>,
    /// When the storage write probe last succeeded.
    pub write_probe: Mutex<Option<Instant>>,
}

impl Guard {
    /// Time left until `client` may present credentials again, if it is over budget.
    pub fn blocked_for(&self, client: IpAddr, limits: &Limits) -> Option<Duration> {
        let failures = self.failures.lock().unwrap_or_else(|e| e.into_inner());
        let (count, since) = failures.get(&client)?;
        let remaining = limits.auth_window.checked_sub(since.elapsed())?;
        (*count >= limits.auth_failures).then_some(remaining)
    }

    pub fn record_failure(&self, client: IpAddr, limits: &Limits) {
        let mut failures = self.failures.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        if failures.len() >= MAX_TRACKED_CLIENTS {
            failures.retain(|_, (_, since)| now.duration_since(*since) < limits.auth_window);
            if failures.len() >= MAX_TRACKED_CLIENTS {
                return;
            }
        }
        let entry = failures.entry(client).or_insert((0, now));
        if now.duration_since(entry.1) >= limits.auth_window {
            *entry = (0, now);
        }
        entry.0 = entry.0.saturating_add(1);
    }
}

/// Address used to attribute failures. Behind a trusted proxy this is the
/// rightmost `X-Forwarded-For` entry, the one the proxy itself appended; the
/// left-hand entries are client-supplied and ignored.
pub fn client_address(request: &Request, trust_proxy: bool) -> IpAddr {
    if trust_proxy && let Some(ip) = forwarded_for(request.headers()) {
        return ip;
    }
    request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|info| info.0.ip())
        .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED))
}
fn forwarded_for(headers: &HeaderMap) -> Option<IpAddr> {
    headers
        .get_all("x-forwarded-for")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .next_back()
        .and_then(|entry| entry.trim().parse().ok())
}

fn presents_credentials(headers: &HeaderMap) -> bool {
    headers.contains_key("x-nuget-apikey") || headers.contains_key(header::AUTHORIZATION)
}

/// Rejects requests from clients that keep presenting invalid credentials.
/// Anonymous requests are never counted or blocked: NuGet clients send one
/// unauthenticated probe before retrying with credentials, and public reads
/// must keep working.
pub async fn limit_auth_failures(
    axum::extract::State(state): axum::extract::State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let limits = &state.config.limits;
    if limits.auth_failures == 0 || !presents_credentials(request.headers()) {
        return next.run(request).await;
    }
    let client = client_address(&request, limits.trust_proxy_headers);
    if let Some(wait) = state.guard.blocked_for(client, limits) {
        metrics::counter!(AUTH_BLOCKED).increment(1);
        tracing::warn!(%client, "too many rejected credentials; request refused");
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [(header::RETRY_AFTER, wait.as_secs().max(1).to_string())],
            "Too many rejected credentials; retry later",
        )
            .into_response();
    }
    let response = next.run(request).await;
    if response.status() == StatusCode::UNAUTHORIZED {
        state.guard.record_failure(client, limits);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits(failures: u32) -> Limits {
        Limits {
            auth_failures: failures,
            ..Limits::default()
        }
    }

    #[test]
    fn blocks_after_the_budget_is_spent() {
        let guard = Guard::default();
        let limits = limits(3);
        let client: IpAddr = "203.0.113.7".parse().unwrap();
        for _ in 0..2 {
            guard.record_failure(client, &limits);
            assert!(guard.blocked_for(client, &limits).is_none());
        }
        guard.record_failure(client, &limits);
        assert!(guard.blocked_for(client, &limits).is_some());
        let other: IpAddr = "203.0.113.8".parse().unwrap();
        assert!(guard.blocked_for(other, &limits).is_none());
    }

    #[test]
    fn window_expiry_clears_the_block() {
        let guard = Guard::default();
        let limits = Limits {
            auth_failures: 1,
            auth_window: Duration::from_millis(30),
            ..Limits::default()
        };
        let client: IpAddr = "::1".parse().unwrap();
        guard.record_failure(client, &limits);
        assert!(guard.blocked_for(client, &limits).is_some());
        std::thread::sleep(Duration::from_millis(40));
        assert!(guard.blocked_for(client, &limits).is_none());
        guard.record_failure(client, &limits);
        assert!(guard.blocked_for(client, &limits).is_some());
    }

    #[test]
    fn proxy_address_is_the_rightmost_forwarded_entry() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-forwarded-for",
            "198.51.100.1, 203.0.113.9".parse().unwrap(),
        );
        assert_eq!(forwarded_for(&headers), "203.0.113.9".parse().ok());
        headers.insert("x-forwarded-for", "not-an-ip".parse().unwrap());
        assert_eq!(forwarded_for(&headers), None);
    }
}
