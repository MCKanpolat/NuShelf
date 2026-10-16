use crate::error::{ApiError, ApiResult};
use serde::Deserialize;
use sqlx::FromRow;
use uuid::Uuid;
#[derive(FromRow)]
pub(super) struct Hook {
    pub(super) id: Uuid,
    pub(super) name: String,
    pub(super) url: String,
    pub(super) secret_env: String,
    pub(super) events: Vec<String>,
    pub(super) feed_id: Option<Uuid>,
    pub(super) enabled: bool,
    pub(super) allow_http: bool,
    pub(super) allow_private_network: bool,
    pub(super) revision: i64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HookInput {
    pub(super) name: String,
    pub(super) url: String,
    pub(super) secret_env: String,
    pub(super) events: Vec<String>,
    pub(super) feed_id: Option<Uuid>,
    #[serde(default = "enabled")]
    pub(super) enabled: bool,
    #[serde(default)]
    pub(super) allow_http: bool,
    #[serde(default)]
    pub(super) allow_private_network: bool,
}
fn enabled() -> bool {
    true
}
pub(super) fn validate(input: &HookInput) -> ApiResult<()> {
    let url = url::Url::parse(&input.url).map_err(|_| ApiError::bad("Invalid webhook URL"))?;
    if input.name.trim().is_empty()
        || input.name.len() > 80
        || input.name.chars().any(char::is_control)
        || !matches!(url.scheme(), "https" | "http")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.query().is_some()
        || (url.scheme() == "http" && !input.allow_http)
    {
        return Err(ApiError::bad(
            "Webhook needs a name and safe HTTPS URL (HTTP requires explicit permission)",
        ));
    }
    if input.secret_env.len() > 128
        || !input.secret_env.starts_with("WEBHOOK_")
        || !input
            .secret_env
            .bytes()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_')
    {
        return Err(ApiError::bad(
            "Signing secret must reference a WEBHOOK_ environment variable",
        ));
    }
    if input.events.is_empty()
        || input.events.len() > 20
        || input.events.iter().any(|e| {
            ![
                "*",
                "package.published",
                "package.unlisted",
                "package.relisted",
                "symbols.published",
                "symbols.deleted",
                "credential.created",
                "credential.revoked",
                "upstream.changed",
                "cache.changed",
                "management.changed",
            ]
            .contains(&e.as_str())
        })
    {
        return Err(ApiError::bad("Unknown webhook event type"));
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInput {
    pub(super) revision: i64,
    #[serde(flatten)]
    pub(super) config: HookInput,
}
impl Hook {
    pub(super) fn matches(&self, payload: &serde_json::Value) -> bool {
        (self.feed_id.is_none()
            || self.feed_id.map(|id| id.to_string())
                == payload["feedId"].as_str().map(str::to_owned))
            && self
                .events
                .iter()
                .any(|event| event == "*" || Some(event.as_str()) == payload["type"].as_str())
    }
}
