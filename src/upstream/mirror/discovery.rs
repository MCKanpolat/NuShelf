use super::model::{Discovered, Discovery};
use crate::{
    AppState,
    error::{ApiError, ApiResult},
    upstream::{http, sources::Source},
    version::{NuGetVersion, package_id},
};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
async fn required(source: &Source, url: &str) -> ApiResult<Value> {
    http::json(source, url)
        .await?
        .ok_or_else(|| http::failure("Mirror discovery document was not found"))
}
fn timestamp(value: &Value) -> ApiResult<DateTime<Utc>> {
    value
        .as_str()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|t| t.with_timezone(&Utc))
        .ok_or_else(|| http::failure("Mirror catalog contains an invalid commit timestamp"))
}
pub(super) async fn page(
    state: &AppState,
    source: &Source,
    cursor: &Value,
) -> ApiResult<Discovery> {
    let index = http::index(source).await?;
    if http::has_resource(&index, "Catalog") {
        catalog(state, source, &index, cursor).await
    } else {
        search(state, source, &index, cursor).await
    }
}
async fn catalog(
    state: &AppState,
    source: &Source,
    index: &Value,
    cursor: &Value,
) -> ApiResult<Discovery> {
    let url = http::resource_endpoint_for(state, index, "Catalog")?;
    let catalog = required(source, &url).await?;
    let pages = catalog["items"]
        .as_array()
        .ok_or_else(|| http::failure("Invalid mirror catalog index"))?;
    let checkpoint = cursor.get("timestamp").map(timestamp).transpose()?;
    let mut pending = Vec::new();
    for p in pages {
        let at = timestamp(&p["commitTimeStamp"])?;
        if checkpoint.is_none_or(|c| at > c) {
            pending.push((at, p));
        }
    }
    pending.sort_by_key(|(at, _)| *at);
    let Some((_, p)) = pending.first() else {
        return Ok(Discovery {
            jobs: Vec::new(),
            cursor: cursor.clone(),
            complete: true,
        });
    };
    let page = required(
        source,
        p["@id"]
            .as_str()
            .ok_or_else(|| http::failure("Missing mirror catalog page URL"))?,
    )
    .await?;
    let items = page["items"]
        .as_array()
        .ok_or_else(|| http::failure("Invalid mirror catalog page"))?;
    if items.len() > 10000 {
        return Err(http::failure("Mirror catalog page exceeds 10000 entries"));
    }
    let high = timestamp(&page["commitTimeStamp"])?;
    if checkpoint.is_some_and(|c| high <= c) {
        return Err(http::failure("Mirror catalog did not advance its cursor"));
    }
    let mut jobs = Vec::new();
    for item in items {
        let at = timestamp(&item["commitTimeStamp"])?;
        if at > high {
            return Err(http::failure(
                "Mirror catalog entry exceeds its page timestamp",
            ));
        }
        if checkpoint.is_some_and(|c| at <= c) {
            continue;
        }
        let id = package_id(item["nuget:id"].as_str().unwrap_or_default())
            .map_err(|_| http::failure("Invalid mirror package ID"))?;
        let version = NuGetVersion::parse(item["nuget:version"].as_str().unwrap_or_default())
            .map_err(|_| http::failure("Invalid mirror package version"))?
            .normalized;
        let kind = item["@type"]
            .as_str()
            .ok_or_else(|| http::failure("Missing mirror catalog event type"))?;
        let action = match kind {
            "nuget:PackageDetails" => "sync",
            "nuget:PackageDelete" => "delete",
            _ => return Err(http::failure("Unsupported mirror catalog event type")),
        };
        jobs.push(Discovered {
            id,
            version,
            action,
            at,
        });
    }
    Ok(Discovery {
        jobs,
        cursor: json!({"kind":"catalog","timestamp":high}),
        complete: false,
    })
}
async fn search(
    state: &AppState,
    source: &Source,
    index: &Value,
    cursor: &Value,
) -> ApiResult<Discovery> {
    let skip = cursor["skip"].as_u64().unwrap_or(0);
    let mut url = url::Url::parse(&http::resource_endpoint_for(
        state,
        index,
        "SearchQueryService",
    )?)
    .map_err(ApiError::internal)?;
    url.query_pairs_mut().extend_pairs([
        ("q", ""),
        ("skip", &skip.to_string()),
        ("take", "100"),
        ("prerelease", "true"),
        ("semVerLevel", "2.0.0"),
    ]);
    let page = required(source, url.as_str()).await?;
    let items = page["data"]
        .as_array()
        .ok_or_else(|| http::failure("Invalid mirror search response"))?;
    let total = page["totalHits"]
        .as_u64()
        .ok_or_else(|| http::failure("Invalid mirror search count"))?;
    if items.len() > 100 || items.is_empty() && skip < total {
        return Err(http::failure("Mirror search paging did not advance"));
    }
    let mut jobs = Vec::new();
    for item in items {
        let id = package_id(item["id"].as_str().unwrap_or_default())
            .map_err(|_| http::failure("Invalid mirror search package ID"))?;
        jobs.push(Discovered {
            id,
            version: String::new(),
            action: "sync",
            at: Utc::now(),
        });
    }
    let next = skip + items.len() as u64;
    Ok(Discovery {
        jobs,
        cursor: json!({"kind":"search","skip":next}),
        complete: next >= total,
    })
}
