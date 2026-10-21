use crate::{
    AppState,
    error::{ApiError, ApiResult},
    upstream::{http, sources::Source},
    version::NuGetVersion,
};
use serde_json::{Value, json};

/// Strict origin reads: never update metadata, pins, cache or download counts.
pub async fn versions(
    state: &AppState,
    source: &Source,
    id: &str,
    version: &str,
) -> ApiResult<Value> {
    let index = http::fetch_index(source).await?;
    let base = http::resource_for(state, &index, "PackageBaseAddress")?;
    let Some(value) = http::json(source, &format!("{base}{id}/index.json")).await? else {
        return Ok(json!({"code":"package_missing","continue":true}));
    };
    let values = value["versions"]
        .as_array()
        .filter(|v| !v.is_empty() && v.len() <= 10000)
        .ok_or_else(|| http::failure("Invalid upstream version index"))?;
    let mut found = false;
    for value in values {
        let raw = value
            .as_str()
            .ok_or_else(|| http::failure("Invalid upstream version"))?;
        let parsed =
            NuGetVersion::parse(raw).map_err(|_| ApiError::bad("Invalid upstream version"))?;
        found |= parsed.normalized == version;
    }
    Ok(
        json!({"code":if found {"version_available"} else {"version_missing"},"continue":false,"archiveVerified":false}),
    )
}
