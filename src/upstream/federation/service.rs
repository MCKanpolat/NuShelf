use super::model::{Query, SourcePage};
use crate::{
    AppState,
    error::ApiResult,
    upstream::{
        cache, http,
        sources::{self, Reader, SELECT_SOURCE, Source},
    },
    version::{NuGetVersion, package_id},
};
use futures_util::{StreamExt, stream};
use serde_json::{Value, json};
use std::collections::HashSet;
fn visible(reader: &Reader, source: &Source) -> bool {
    source.enabled
        && (!source.private
            || reader.admin
            || reader
                .profile
                .is_some_and(|p| source.allowed_profiles.contains(&p)))
}
pub async fn search(state: &AppState, reader: &Reader, query: &Query) -> ApiResult<Value> {
    query.validate()?;
    let all = sqlx::query_as::<_, Source>(&format!(
        "{SELECT_SOURCE} WHERE enabled ORDER BY priority,id"
    ))
    .fetch_all(&state.db)
    .await?;
    let selected = all
        .iter()
        .filter(|s| visible(reader, s) && query.source_id.is_none_or(|id| s.id == id))
        .cloned()
        .collect::<Vec<_>>();
    if query.source_id.is_some() && selected.is_empty() {
        return Err(crate::error::ApiError::not_found());
    }
    let mut pages = stream::iter(selected)
        .map(|source| async {
            let result = fetch(state, reader, query, &source, &all).await;
            match result {
                Ok(page) => page,
                Err(error) => SourcePage {
                    source_id: source.id,
                    name: source.name,
                    upstream_total_hits: None,
                    skip: query.skip,
                    next_skip: None,
                    data: Vec::new(),
                    error: Some(error.1),
                },
            }
        })
        .buffered(4)
        .collect::<Vec<_>>()
        .await;
    // Stable priority order and one visible source per ID across this batch.
    let mut seen = HashSet::new();
    for page in &mut pages {
        page.data
            .retain(|v| seen.insert(v["id"].as_str().unwrap_or_default().to_ascii_lowercase()));
    }
    let partial = pages.iter().any(|p| p.error.is_some());
    Ok(json!({"query":query.q,"sources":pages,"partial":partial}))
}
async fn fetch(
    state: &AppState,
    reader: &Reader,
    query: &Query,
    source: &Source,
    all: &[Source],
) -> ApiResult<SourcePage> {
    let index = http::index(source).await?;
    let mut url = url::Url::parse(&http::resource_endpoint_for(
        state,
        &index,
        "SearchQueryService",
    )?)
    .map_err(crate::error::ApiError::internal)?;
    let take = query.take.unwrap_or(20);
    url.query_pairs_mut().extend_pairs([
        ("q", query.q.trim()),
        ("skip", &query.skip.to_string()),
        ("take", &take.to_string()),
        (
            "prerelease",
            if query.prerelease { "true" } else { "false" },
        ),
        (
            "semVerLevel",
            query.sem_ver_level.as_deref().unwrap_or("1.0.0"),
        ),
    ]);
    let result = http::json(source, url.as_str())
        .await?
        .ok_or_else(|| http::failure("Upstream search endpoint was not found"))?;
    let data = result["data"]
        .as_array()
        .ok_or_else(|| http::failure("Invalid upstream search response"))?;
    let total = result["totalHits"]
        .as_u64()
        .ok_or_else(|| http::failure("Invalid upstream search count"))?;
    if data.len() > take as usize || data.is_empty() && (query.skip as u64) < total {
        return Err(http::failure("Upstream search returned an invalid page"));
    }
    let mut cleaned = Vec::new();
    let semver2 = crate::registry::channel(&query.local()) >= 2;
    for raw in data {
        let id = package_id(raw["id"].as_str().unwrap_or_default())
            .map_err(|_| http::failure("Invalid upstream search ID"))?;
        if !id.starts_with(&state.read_prefix) || reader.check(source, &id).is_err() {
            continue;
        }
        if !sources::select_candidates(all.to_vec(), &id)
            .iter()
            .any(|s| s.id == source.id)
        {
            continue;
        }
        let local: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM packages WHERE id=$1)")
            .bind(&id)
            .fetch_one(&state.db)
            .await?;
        if local {
            continue;
        }
        let pin: Option<uuid::Uuid> =
            sqlx::query_scalar("SELECT source_id FROM upstream_metadata WHERE package_id=$1")
                .bind(&id)
                .fetch_optional(&state.db)
                .await?;
        if pin.is_some_and(|p| p != source.id) {
            continue;
        }
        let v = NuGetVersion::parse(raw["version"].as_str().unwrap_or_default())
            .map_err(|_| http::failure("Invalid upstream search version"))?;
        if (!query.prerelease && v.prerelease) || (!semver2 && v.semver2) || raw["listed"] == false
        {
            continue;
        }
        let mut item = cache::clean_catalog(state, &id, raw)?;
        item["authors"] = json!(match raw["authors"].as_array() {
            Some(v) => v.clone(),
            None => vec![raw["authors"].clone()],
        });
        item["totalDownloads"] = json!(raw["totalDownloads"].as_u64().unwrap_or(0));
        item["registration"] = json!(format!(
            "{}/v3/registration/{id}/index.json",
            state.config.public_url
        ));
        item["upstream"] = json!({"id":source.id,"name":source.name,"private":source.private});
        item["cacheStatus"] = json!("metadataOnly");
        let mut versions = Vec::new();
        for raw_version in raw["versions"].as_array().into_iter().flatten() {
            let version = NuGetVersion::parse(raw_version["version"].as_str().unwrap_or_default())
                .map_err(|_| http::failure("Invalid upstream search version list"))?;
            if !query.prerelease && version.prerelease || !semver2 && version.semver2 {
                continue;
            }
            versions.push(json!({"version":version.display,"downloads":raw_version["downloads"].as_u64().unwrap_or(0),"@id":format!("{}/v3/registration/{id}/{}.json",state.config.public_url,version.normalized)}));
        }
        if versions.is_empty() {
            versions.push(json!({"version":v.display,"downloads":0,"@id":item["@id"]}));
        }
        item["versions"] = json!(versions);
        cleaned.push(item);
    }
    let next = query.skip + data.len() as i64;
    Ok(SourcePage {
        source_id: source.id,
        name: source.name.clone(),
        upstream_total_hits: Some(total),
        skip: query.skip,
        next_skip: ((next as u64) < total).then_some(next),
        data: cleaned,
        error: None,
    })
}
