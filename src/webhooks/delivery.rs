use super::model::Hook;
use crate::{
    error::{ApiError, ApiResult},
    upstream::sources::Source,
};
use hmac::{Hmac, Mac};
use serde_json::Value;
use sha2::Sha256;
use uuid::Uuid;
pub fn signature(secret: &str, timestamp: i64, body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
        .expect("HMAC accepts arbitrary key sizes");
    mac.update(format!("{timestamp}.").as_bytes());
    mac.update(body);
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}
pub(super) async fn send(hook: &Hook, delivery: Uuid, payload: &Value) -> ApiResult<u16> {
    let secret = std::env::var(&hook.secret_env)
        .ok()
        .filter(|s| s.len() >= 32)
        .ok_or_else(|| {
            ApiError::bad("Webhook signing secret is missing or shorter than 32 characters")
        })?;
    let source = Source {
        id: hook.id,
        name: hook.name.clone(),
        url: hook.url.clone(),
        priority: 0,
        enabled: true,
        patterns: vec![],
        excludes: vec![],
        private: false,
        auth_type: "none".into(),
        username: None,
        credential_env: None,
        allow_http: hook.allow_http,
        allow_private_network: hook.allow_private_network,
        timeout_seconds: 10,
        metadata_ttl_seconds: 1,
        stale_on_error: false,
        service_index: None,
        index_checked_at: None,
        allowed_profiles: vec![],
    };
    let url = url::Url::parse(&hook.url).map_err(ApiError::internal)?;
    let client = crate::upstream::http::client(&source, &url).await?;
    let body = serde_json::to_vec(payload).map_err(ApiError::internal)?;
    let timestamp = chrono::Utc::now().timestamp();
    let response = client
        .post(url)
        .header("content-type", "application/json")
        .header("x-nushelf-delivery", delivery.to_string())
        .header("x-nushelf-timestamp", timestamp.to_string())
        .header("x-nushelf-signature", signature(&secret, timestamp, &body))
        .header(
            "x-nushelf-event",
            payload["type"].as_str().unwrap_or("management.changed"),
        )
        .body(body)
        .send()
        .await
        .map_err(|_| ApiError::bad("Webhook connection failed"))?;
    Ok(response.status().as_u16())
}
